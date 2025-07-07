use std::{
    any::TypeId, borrow::Cow, collections::HashMap, future::Ready, marker::PhantomData, sync::Arc,
};

use futures::future::{BoxFuture, FutureExt};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio_util::sync::CancellationToken;

pub use super::router::tool::{ToolRoute, ToolRouter};
use crate::{
    RoleServer,
    model::{CallToolRequestParam, CallToolResult, IntoContents, JsonObject},
    service::RequestContext,
};

/// A shortcut for generating a JSON schema for a type.
pub fn schema_for_type<T: JsonSchema>() -> JsonObject {
    // explicitly to align json schema version to official specifications.
    // https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/schema/2025-03-26/schema.json
    let mut settings = schemars::r#gen::SchemaSettings::draft07();
    settings.option_nullable = true;
    settings.option_add_null_type = false;
    settings.visitors = Vec::default();
    let generator = settings.into_generator();
    let schema = generator.into_root_schema_for::<T>();
    let object = serde_json::to_value(schema).expect("failed to serialize schema");
    match object {
        serde_json::Value::Object(object) => object,
        _ => panic!("unexpected schema value"),
    }
}

/// Call [`schema_for_type`] with a cache
pub fn cached_schema_for_type<T: JsonSchema + std::any::Any>() -> Arc<JsonObject> {
    thread_local! {
        static CACHE_FOR_TYPE: std::sync::RwLock<HashMap<TypeId, Arc<JsonObject>>> = Default::default();
    };
    CACHE_FOR_TYPE.with(|cache| {
        if let Some(x) = cache
            .read()
            .expect("schema cache lock poisoned")
            .get(&TypeId::of::<T>())
        {
            x.clone()
        } else {
            let schema = schema_for_type::<T>();
            let schema = Arc::new(schema);
            cache
                .write()
                .expect("schema cache lock poisoned")
                .insert(TypeId::of::<T>(), schema.clone());
            schema
        }
    })
}

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
}

impl<'s, S> ToolCallContext<'s, S> {
    pub fn new(
        service: &'s S,
        CallToolRequestParam { name, arguments }: CallToolRequestParam,
        request_context: RequestContext<RoleServer>,
    ) -> Self {
        Self {
            request_context,
            service,
            name,
            arguments,
        }
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn request_context(&self) -> &RequestContext<RoleServer> {
        &self.request_context
    }
}

pub trait FromToolCallContextPart<S>: Sized {
    fn from_tool_call_context_part(
        context: &mut ToolCallContext<S>,
    ) -> Result<Self, crate::ErrorData>;
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
    ) -> BoxFuture<'_, Result<CallToolResult, crate::ErrorData>>;
}

pub type DynCallToolHandler<S> = dyn for<'s> Fn(ToolCallContext<'s, S>) -> BoxFuture<'s, Result<CallToolResult, crate::ErrorData>>
    + Send
    + Sync;

/// Parameter Extractor
///
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Parameters<P>(pub P);

impl<P: JsonSchema> JsonSchema for Parameters<P> {
    fn schema_name() -> String {
        P::schema_name()
    }

    fn json_schema(generator: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
        P::json_schema(generator)
    }
}

impl<S> FromToolCallContextPart<S> for CancellationToken {
    fn from_tool_call_context_part(
        context: &mut ToolCallContext<S>,
    ) -> Result<Self, crate::ErrorData> {
        Ok(context.request_context.ct.clone())
    }
}

pub struct ToolName(pub Cow<'static, str>);

impl<S> FromToolCallContextPart<S> for ToolName {
    fn from_tool_call_context_part(
        context: &mut ToolCallContext<S>,
    ) -> Result<Self, crate::ErrorData> {
        Ok(Self(context.name.clone()))
    }
}

impl<S, P> FromToolCallContextPart<S> for Parameters<P>
where
    P: DeserializeOwned,
{
    fn from_tool_call_context_part(
        context: &mut ToolCallContext<S>,
    ) -> Result<Self, crate::ErrorData> {
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

impl<S> FromToolCallContextPart<S> for JsonObject {
    fn from_tool_call_context_part(
        context: &mut ToolCallContext<S>,
    ) -> Result<Self, crate::ErrorData> {
        let object = context.arguments.take().unwrap_or_default();
        Ok(object)
    }
}

impl<S> FromToolCallContextPart<S> for crate::model::Extensions {
    fn from_tool_call_context_part(
        context: &mut ToolCallContext<S>,
    ) -> Result<Self, crate::ErrorData> {
        let extensions = context.request_context.extensions.clone();
        Ok(extensions)
    }
}

pub struct Extension<T>(pub T);

impl<S, T> FromToolCallContextPart<S> for Extension<T>
where
    T: Send + Sync + 'static + Clone,
{
    fn from_tool_call_context_part(
        context: &mut ToolCallContext<S>,
    ) -> Result<Self, crate::ErrorData> {
        let extension = context
            .request_context
            .extensions
            .get::<T>()
            .cloned()
            .ok_or_else(|| {
                crate::ErrorData::invalid_params(
                    format!("missing extension {}", std::any::type_name::<T>()),
                    None,
                )
            })?;
        Ok(Extension(extension))
    }
}

impl<S> FromToolCallContextPart<S> for crate::Peer<RoleServer> {
    fn from_tool_call_context_part(
        context: &mut ToolCallContext<S>,
    ) -> Result<Self, crate::ErrorData> {
        let peer = context.request_context.peer.clone();
        Ok(peer)
    }
}

impl<S> FromToolCallContextPart<S> for crate::model::Meta {
    fn from_tool_call_context_part(
        context: &mut ToolCallContext<S>,
    ) -> Result<Self, crate::ErrorData> {
        let mut meta = crate::model::Meta::default();
        std::mem::swap(&mut meta, &mut context.request_context.meta);
        Ok(meta)
    }
}

pub struct RequestId(pub crate::model::RequestId);
impl<S> FromToolCallContextPart<S> for RequestId {
    fn from_tool_call_context_part(
        context: &mut ToolCallContext<S>,
    ) -> Result<Self, crate::ErrorData> {
        Ok(RequestId(context.request_context.id.clone()))
    }
}

impl<S> FromToolCallContextPart<S> for RequestContext<RoleServer> {
    fn from_tool_call_context_part(
        context: &mut ToolCallContext<S>,
    ) -> Result<Self, crate::ErrorData> {
        Ok(context.request_context.clone())
    }
}

impl<'s, S> ToolCallContext<'s, S> {
    pub fn invoke<H, A>(self, h: H) -> BoxFuture<'s, Result<CallToolResult, crate::ErrorData>>
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
                $Tn: FromToolCallContextPart<S> ,
            )*
            F: FnOnce(&S, $($Tn,)*) -> BoxFuture<'_, R>,

            // Need RTN support here(I guess), https://github.com/rust-lang/rust/pull/138424
            // Fut: Future<Output = R> + Send + 'a,
            R: IntoCallToolResult + Send + 'static,
            S: Send + Sync + 'static,
        {
            #[allow(unused_variables, non_snake_case, unused_mut)]
            fn call(
                self,
                mut context: ToolCallContext<'_, S>,
            ) -> BoxFuture<'_, Result<CallToolResult, crate::ErrorData>>{
                $(
                    let result = $Tn::from_tool_call_context_part(&mut context);
                    let $Tn = match result {
                        Ok(value) => value,
                        Err(e) => return std::future::ready(Err(e)).boxed(),
                    };
                )*
                let service = context.service;
                let fut = self(service, $($Tn,)*);
                async move {
                    let result = fut.await;
                    result.into_call_tool_result()
                }.boxed()
            }
        }

        impl<$($Tn,)* S, F, Fut, R> CallToolHandler<S, AsyncAdapter<($($Tn,)*), Fut, R>> for F
        where
            $(
                $Tn: FromToolCallContextPart<S> ,
            )*
            F: FnOnce($($Tn,)*) -> Fut + Send + ,
            Fut: Future<Output = R> + Send + 'static,
            R: IntoCallToolResult + Send + 'static,
            S: Send + Sync,
        {
            #[allow(unused_variables, non_snake_case, unused_mut)]
            fn call(
                self,
                mut context: ToolCallContext<S>,
            ) -> BoxFuture<'static, Result<CallToolResult, crate::ErrorData>>{
                $(
                    let result = $Tn::from_tool_call_context_part(&mut context);
                    let $Tn = match result {
                        Ok(value) => value,
                        Err(e) => return std::future::ready(Err(e)).boxed(),
                    };
                )*
                let fut = self($($Tn,)*);
                async move {
                    let result = fut.await;
                    result.into_call_tool_result()
                }.boxed()
            }
        }

        impl<$($Tn,)* S, F, R> CallToolHandler<S, SyncMethodAdapter<($($Tn,)*), R>> for F
        where
            $(
                $Tn: FromToolCallContextPart<S> + ,
            )*
            F: FnOnce(&S, $($Tn,)*) -> R + Send + ,
            R: IntoCallToolResult + Send + ,
            S: Send + Sync,
        {
            #[allow(unused_variables, non_snake_case, unused_mut)]
            fn call(
                self,
                mut context: ToolCallContext<S>,
            ) -> BoxFuture<'static, Result<CallToolResult, crate::ErrorData>> {
                $(
                    let result = $Tn::from_tool_call_context_part(&mut context);
                    let $Tn = match result {
                        Ok(value) => value,
                        Err(e) => return std::future::ready(Err(e)).boxed(),
                    };
                )*
                std::future::ready(self(context.service, $($Tn,)*).into_call_tool_result()).boxed()
            }
        }

        impl<$($Tn,)* S, F, R> CallToolHandler<S, SyncAdapter<($($Tn,)*), R>> for F
        where
            $(
                $Tn: FromToolCallContextPart<S> + ,
            )*
            F: FnOnce($($Tn,)*) -> R + Send + ,
            R: IntoCallToolResult + Send + ,
            S: Send + Sync,
        {
            #[allow(unused_variables, non_snake_case, unused_mut)]
            fn call(
                self,
                mut context: ToolCallContext<S>,
            ) -> BoxFuture<'static, Result<CallToolResult, crate::ErrorData>>  {
                $(
                    let result = $Tn::from_tool_call_context_part(&mut context);
                    let $Tn = match result {
                        Ok(value) => value,
                        Err(e) => return std::future::ready(Err(e)).boxed(),
                    };
                )*
                std::future::ready(self($($Tn,)*).into_call_tool_result()).boxed()
            }
        }
    };
}
impl_for!(T0 T1 T2 T3 T4 T5 T6 T7 T8 T9 T10 T11 T12 T13 T14 T15);
