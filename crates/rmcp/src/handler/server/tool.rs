use std::{
    borrow::Cow,
    future::{Future, Ready},
    marker::PhantomData,
};

#[cfg(not(feature = "local"))]
use futures::future::BoxFuture;
use serde::de::DeserializeOwned;

use super::common::{AsRequestContext, FromContextPart};
pub use super::{
    common::{Extension, RequestId, schema_for_output, schema_for_type},
    router::tool::{ToolRoute, ToolRouter},
};
use crate::{
    RoleServer,
    handler::server::wrapper::Parameters,
    model::{CallToolRequestParams, CallToolResult, IntoContents, JsonObject},
    service::{MaybeBoxFuture, MaybeSend, MaybeSendFuture, RequestContext},
};

/// Deserialize a JSON object into a type
pub fn parse_json_object<T: DeserializeOwned>(input: JsonObject) -> Result<T, crate::ErrorData> {
    serde_json::from_value(serde_json::Value::Object(input)).map_err(|e| {
        crate::ErrorData::invalid_params(
            format!("failed to deserialize parameters: {error}", error = e),
            None,
        )
    })
}
pub struct ToolCallContext<'s, S> {
    pub request_context: RequestContext<RoleServer>,
    pub service: &'s S,
    pub name: Cow<'static, str>,
    pub arguments: Option<JsonObject>,
    pub task: Option<JsonObject>,
}

impl<'s, S> ToolCallContext<'s, S> {
    pub fn new(
        service: &'s S,
        CallToolRequestParams {
            meta: _,
            name,
            arguments,
            task,
        }: CallToolRequestParams,
        request_context: RequestContext<RoleServer>,
    ) -> Self {
        Self {
            request_context,
            service,
            name,
            arguments,
            task,
        }
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn request_context(&self) -> &RequestContext<RoleServer> {
        &self.request_context
    }
}

impl<S> AsRequestContext for ToolCallContext<'_, S> {
    fn as_request_context(&self) -> &RequestContext<RoleServer> {
        &self.request_context
    }

    fn as_request_context_mut(&mut self) -> &mut RequestContext<RoleServer> {
        &mut self.request_context
    }
}

pub trait IntoCallToolResult {
    fn into_call_tool_result(self) -> Result<CallToolResult, crate::ErrorData>;
}

impl<T: IntoContents> IntoCallToolResult for T {
    fn into_call_tool_result(self) -> Result<CallToolResult, crate::ErrorData> {
        Ok(CallToolResult::success(self.into_contents()))
    }
}

impl<T: IntoContents, E: IntoContents> IntoCallToolResult for Result<T, E> {
    fn into_call_tool_result(self) -> Result<CallToolResult, crate::ErrorData> {
        match self {
            Ok(value) => Ok(CallToolResult::success(value.into_contents())),
            Err(error) => Ok(CallToolResult::error(error.into_contents())),
        }
    }
}

impl<T: IntoCallToolResult> IntoCallToolResult for Result<T, crate::ErrorData> {
    fn into_call_tool_result(self) -> Result<CallToolResult, crate::ErrorData> {
        match self {
            Ok(value) => value.into_call_tool_result(),
            Err(error) => Err(error),
        }
    }
}

pin_project_lite::pin_project! {
    #[project = IntoCallToolResultFutProj]
    pub enum IntoCallToolResultFut<F, R> {
        Pending {
            #[pin]
            fut: F,
            _marker: PhantomData<R>,
        },
        Ready {
            #[pin]
            result: Ready<Result<CallToolResult, crate::ErrorData>>,
        }
    }
}

impl<F, R> Future for IntoCallToolResultFut<F, R>
where
    F: Future<Output = R>,
    R: IntoCallToolResult,
{
    type Output = Result<CallToolResult, crate::ErrorData>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        match self.project() {
            IntoCallToolResultFutProj::Pending { fut, _marker } => {
                fut.poll(cx).map(IntoCallToolResult::into_call_tool_result)
            }
            IntoCallToolResultFutProj::Ready { result } => result.poll(cx),
        }
    }
}

impl IntoCallToolResult for Result<CallToolResult, crate::ErrorData> {
    fn into_call_tool_result(self) -> Result<CallToolResult, crate::ErrorData> {
        self
    }
}

pub trait CallToolHandler<S, A> {
    fn call(
        self,
        context: ToolCallContext<'_, S>,
    ) -> MaybeBoxFuture<'_, Result<CallToolResult, crate::ErrorData>>;
}

#[cfg(not(feature = "local"))]
pub type DynCallToolHandler<S> = dyn for<'s> Fn(ToolCallContext<'s, S>) -> BoxFuture<'s, Result<CallToolResult, crate::ErrorData>>
    + Send
    + Sync;

#[cfg(feature = "local")]
pub type DynCallToolHandler<S> =
    dyn for<'s> Fn(
        ToolCallContext<'s, S>,
    )
        -> futures::future::LocalBoxFuture<'s, Result<CallToolResult, crate::ErrorData>>;

// Tool-specific extractor for tool name
pub struct ToolName(pub Cow<'static, str>);

impl<S> FromContextPart<ToolCallContext<'_, S>> for ToolName {
    fn from_context_part(context: &mut ToolCallContext<S>) -> Result<Self, crate::ErrorData> {
        Ok(Self(context.name.clone()))
    }
}

// Special implementation for Parameters that handles tool arguments
impl<S, P> FromContextPart<ToolCallContext<'_, S>> for Parameters<P>
where
    P: DeserializeOwned,
{
    fn from_context_part(context: &mut ToolCallContext<S>) -> Result<Self, crate::ErrorData> {
        let arguments = context.arguments.take().unwrap_or_default();
        let value: P =
            serde_json::from_value(serde_json::Value::Object(arguments)).map_err(|e| {
                crate::ErrorData::invalid_params(
                    format!("failed to deserialize parameters: {error}", error = e),
                    None,
                )
            })?;
        Ok(Parameters(value))
    }
}

// Special implementation for JsonObject that takes tool arguments
impl<S> FromContextPart<ToolCallContext<'_, S>> for JsonObject {
    fn from_context_part(context: &mut ToolCallContext<S>) -> Result<Self, crate::ErrorData> {
        let object = context.arguments.take().unwrap_or_default();
        Ok(object)
    }
}

impl<'s, S> ToolCallContext<'s, S> {
    pub fn invoke<H, A>(self, h: H) -> MaybeBoxFuture<'s, Result<CallToolResult, crate::ErrorData>>
    where
        H: CallToolHandler<S, A>,
    {
        h.call(self)
    }
}
#[allow(clippy::type_complexity)]
pub struct AsyncAdapter<P, Fut, R>(PhantomData<fn(P) -> fn(Fut) -> R>);
pub struct SyncAdapter<P, R>(PhantomData<fn(P) -> R>);
// #[allow(clippy::type_complexity)]
pub struct AsyncMethodAdapter<P, R>(PhantomData<fn(P) -> R>);
pub struct SyncMethodAdapter<P, R>(PhantomData<fn(P) -> R>);

macro_rules! impl_for {
    ($($T: ident)*) => {
        impl_for!([] [$($T)*]);
    };
    // finished
    ([$($Tn: ident)*] []) => {
        impl_for!(@impl $($Tn)*);
    };
    ([$($Tn: ident)*] [$Tn_1: ident $($Rest: ident)*]) => {
        impl_for!(@impl $($Tn)*);
        impl_for!([$($Tn)* $Tn_1] [$($Rest)*]);
    };
    (@impl $($Tn: ident)*) => {
        impl<$($Tn,)* S, F,  R> CallToolHandler<S, AsyncMethodAdapter<($($Tn,)*), R>> for F
        where
            $(
                $Tn: for<'a> FromContextPart<ToolCallContext<'a, S>> ,
            )*
            F: FnOnce(&S, $($Tn,)*) -> MaybeBoxFuture<'_, R>,

            // Need RTN support here(I guess), https://github.com/rust-lang/rust/pull/138424
            // Fut: Future<Output = R> + Send + 'a,
            R: IntoCallToolResult + MaybeSendFuture + 'static,
            S: MaybeSend + 'static,
        {
            #[allow(unused_variables, non_snake_case, unused_mut)]
            fn call(
                self,
                mut context: ToolCallContext<'_, S>,
            ) -> MaybeBoxFuture<'_, Result<CallToolResult, crate::ErrorData>>{
                $(
                    let result = $Tn::from_context_part(&mut context);
                    let $Tn = match result {
                        Ok(value) => value,
                        Err(e) => return Box::pin(std::future::ready(Err(e))),
                    };
                )*
                let service = context.service;
                let fut = self(service, $($Tn,)*);
                Box::pin(async move {
                    let result = fut.await;
                    result.into_call_tool_result()
                })
            }
        }

        impl<$($Tn,)* S, F, Fut, R> CallToolHandler<S, AsyncAdapter<($($Tn,)*), Fut, R>> for F
        where
            $(
                $Tn: for<'a> FromContextPart<ToolCallContext<'a, S>> ,
            )*
            F: FnOnce($($Tn,)*) -> Fut + MaybeSendFuture,
            Fut: Future<Output = R> + MaybeSendFuture + 'static,
            R: IntoCallToolResult + MaybeSendFuture + 'static,
            S: MaybeSend,
        {
            #[allow(unused_variables, non_snake_case, unused_mut)]
            fn call(
                self,
                mut context: ToolCallContext<S>,
            ) -> MaybeBoxFuture<'static, Result<CallToolResult, crate::ErrorData>>{
                $(
                    let result = $Tn::from_context_part(&mut context);
                    let $Tn = match result {
                        Ok(value) => value,
                        Err(e) => return Box::pin(std::future::ready(Err(e))),
                    };
                )*
                let fut = self($($Tn,)*);
                Box::pin(async move {
                    let result = fut.await;
                    result.into_call_tool_result()
                })
            }
        }

        impl<$($Tn,)* S, F, R> CallToolHandler<S, SyncMethodAdapter<($($Tn,)*), R>> for F
        where
            $(
                $Tn: for<'a> FromContextPart<ToolCallContext<'a, S>> + ,
            )*
            F: FnOnce(&S, $($Tn,)*) -> R + MaybeSendFuture,
            R: IntoCallToolResult + MaybeSendFuture,
            S: MaybeSend,
        {
            #[allow(unused_variables, non_snake_case, unused_mut)]
            fn call(
                self,
                mut context: ToolCallContext<S>,
            ) -> MaybeBoxFuture<'static, Result<CallToolResult, crate::ErrorData>> {
                $(
                    let result = $Tn::from_context_part(&mut context);
                    let $Tn = match result {
                        Ok(value) => value,
                        Err(e) => return Box::pin(std::future::ready(Err(e))),
                    };
                )*
                Box::pin(std::future::ready(self(context.service, $($Tn,)*).into_call_tool_result()))
            }
        }

        impl<$($Tn,)* S, F, R> CallToolHandler<S, SyncAdapter<($($Tn,)*), R>> for F
        where
            $(
                $Tn: for<'a> FromContextPart<ToolCallContext<'a, S>> + ,
            )*
            F: FnOnce($($Tn,)*) -> R + MaybeSendFuture,
            R: IntoCallToolResult + MaybeSendFuture,
            S: MaybeSend,
        {
            #[allow(unused_variables, non_snake_case, unused_mut)]
            fn call(
                self,
                mut context: ToolCallContext<S>,
            ) -> MaybeBoxFuture<'static, Result<CallToolResult, crate::ErrorData>>  {
                $(
                    let result = $Tn::from_context_part(&mut context);
                    let $Tn = match result {
                        Ok(value) => value,
                        Err(e) => return Box::pin(std::future::ready(Err(e))),
                    };
                )*
                Box::pin(std::future::ready(self($($Tn,)*).into_call_tool_result()))
            }
        }
    };
}
impl_for!(T0 T1 T2 T3 T4 T5 T6 T7 T8 T9 T10 T11 T12 T13 T14 T15);
