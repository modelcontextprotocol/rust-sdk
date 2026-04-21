//! Tools for MCP servers.
//!
//! It's straightforward to define tools using [`tool_router`][crate::tool_router] and
//! [`tool`][crate::tool] macro.
//!
//! ```rust
//! # use rmcp::{
//! #     tool_router, tool,
//! #     handler::server::{wrapper::{Parameters, Json}, tool::ToolRouter},
//! #     schemars
//! # };
//! # use serde::{Serialize, Deserialize};
//! struct Server;
//!
//! #[derive(Deserialize, schemars::JsonSchema, Default)]
//! struct AddParameter {
//!     left: usize,
//!     right: usize
//! }
//! #[derive(Serialize, schemars::JsonSchema)]
//! struct AddOutput {
//!     sum: usize
//! }
//! #[tool_router(server_handler)]
//! impl Server {
//!     #[tool(name = "adder", description = "Modular add two integers")]
//!     fn add(
//!         &self,
//!         Parameters(AddParameter { left, right }): Parameters<AddParameter>
//!     ) -> Json<AddOutput> {
//!         Json(AddOutput { sum: left.wrapping_add(right) })
//!     }
//! }
//! ```
//!
//! The `server_handler` flag emits `#[tool_handler]` for you (tools-only servers). For custom
//! `#[tool_handler(...)]` options or multiple handler macros on one `impl ServerHandler`, write
//! `#[tool_router]` and `#[tool_handler] impl ServerHandler for ...` explicitly—see
//! [`tool_router`][crate::tool_router] and [`tool_handler`][crate::tool_handler].
//!
//! Using the macro-based code pattern above is suitable for small MCP servers with simple interfaces.
//! When the business logic become larger, it is recommended that each tool should reside
//! in individual file, combined into MCP server using [`SyncTool`] and [`AsyncTool`] traits.
//!
//! ```rust
//! # use rmcp::{
//! #     handler::server::{
//! #         tool::ToolRouter,
//! #         router::tool::{SyncTool, AsyncTool, ToolBase},
//! #     },
//! #     schemars, ErrorData
//! # };
//! # pub struct MyCustomError;
//! # impl From<MyCustomError> for ErrorData {
//! #     fn from(err: MyCustomError) -> ErrorData { unimplemented!() }
//! # }
//! # use serde::{Serialize, Deserialize};
//! # use std::borrow::Cow;
//! // In tool1.rs
//! pub struct ComplexTool1;
//! #[derive(Deserialize, schemars::JsonSchema, Default)]
//! pub struct ComplexTool1Input { /* ... */ }
//! #[derive(Serialize, schemars::JsonSchema)]
//! pub struct ComplexTool1Output { /* ... */ }
//!
//! impl ToolBase for ComplexTool1 {
//!     type Parameter = ComplexTool1Input;
//!     type Output = ComplexTool1Output;
//!     type Error = MyCustomError;
//!     fn name() -> Cow<'static, str> {
//!         "complex-tool1".into()
//!     }
//!
//!     fn description() -> Option<Cow<'static, str>> {
//!         Some("...".into())
//!     }
//! }
//! impl SyncTool<MyToolServer> for ComplexTool1 {
//!     fn invoke(service: &MyToolServer, param: Self::Parameter) -> Result<Self::Output, Self::Error> {
//!         // ...
//! #       unimplemented!()
//!     }
//! }
//! // In tool2.rs
//! pub struct ComplexTool2;
//! #[derive(Deserialize, schemars::JsonSchema, Default)]
//! pub struct ComplexTool2Input { /* ... */ }
//! #[derive(Serialize, schemars::JsonSchema)]
//! pub struct ComplexTool2Output { /* ... */ }
//!
//! impl ToolBase for ComplexTool2 {
//!     type Parameter = ComplexTool2Input;
//!     type Output = ComplexTool2Output;
//!     type Error = MyCustomError;
//!     fn name() -> Cow<'static, str> {
//!         "complex-tool2".into()
//!     }
//!
//!     fn description() -> Option<Cow<'static, str>> {
//!         Some("...".into())
//!     }
//! }
//! impl AsyncTool<MyToolServer> for ComplexTool2 {
//!     async fn invoke(service: &MyToolServer, param: Self::Parameter) -> Result<Self::Output, Self::Error> {
//!         // ...
//! #       unimplemented!()
//!     }
//! }
//!
//! // In tool_router.rs
//! struct MyToolServer {
//!     tool_router: ToolRouter<Self>,
//! }
//! impl MyToolServer {
//!     pub fn tool_router() -> ToolRouter<Self> {
//!         ToolRouter::new()
//!             .with_sync_tool::<ComplexTool1>()
//!             .with_async_tool::<ComplexTool2>()
//!     }
//! }
//! ```
//!
//! It's also possible to use macro-based and trait-based tool definition together: Since
//! [`ToolRouter`] implements [`Add`][std::ops::Add], you can add two tool routers into final
//! router as showed in [the documentation of `tool_router`][crate::tool_router].

mod tool_traits;

use std::{borrow::Cow, sync::Arc};

use schemars::JsonSchema;
pub use tool_traits::{AsyncTool, SyncTool, ToolBase};

use crate::{
    handler::server::{
        tool::{CallToolHandler, DynCallToolHandler, ToolCallContext, schema_for_type},
        tool_name_validation::validate_and_warn_tool_name,
    },
    model::{CallToolResult, Tool, ToolAnnotations},
    service::{MaybeBoxFuture, MaybeSend},
};

#[non_exhaustive]
pub struct ToolRoute<S> {
    #[allow(clippy::type_complexity)]
    pub call: Arc<DynCallToolHandler<S>>,
    pub attr: crate::model::Tool,
}

impl<S> std::fmt::Debug for ToolRoute<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRoute")
            .field("name", &self.attr.name)
            .field("description", &self.attr.description)
            .field("input_schema", &self.attr.input_schema)
            .finish()
    }
}

impl<S> Clone for ToolRoute<S> {
    fn clone(&self) -> Self {
        Self {
            call: self.call.clone(),
            attr: self.attr.clone(),
        }
    }
}

impl<S: MaybeSend + 'static> ToolRoute<S> {
    pub fn new<C, A>(attr: impl Into<Tool>, call: C) -> Self
    where
        C: CallToolHandler<S, A> + MaybeSend + Clone + 'static,
    {
        Self {
            call: Arc::new(move |context: ToolCallContext<S>| {
                let call = call.clone();
                context.invoke(call)
            }),
            attr: attr.into(),
        }
    }
    pub fn new_dyn<C>(attr: impl Into<Tool>, call: C) -> Self
    where
        C: for<'a> Fn(
                ToolCallContext<'a, S>,
            ) -> MaybeBoxFuture<'a, Result<CallToolResult, crate::ErrorData>>
            + MaybeSend
            + 'static,
    {
        Self {
            call: Arc::new(call),
            attr: attr.into(),
        }
    }
    pub fn name(&self) -> &str {
        &self.attr.name
    }
}

pub trait IntoToolRoute<S, A> {
    fn into_tool_route(self) -> ToolRoute<S>;
}

impl<S, C, A, T> IntoToolRoute<S, A> for (T, C)
where
    S: MaybeSend + 'static,
    C: CallToolHandler<S, A> + MaybeSend + Clone + 'static,
    T: Into<Tool>,
{
    fn into_tool_route(self) -> ToolRoute<S> {
        ToolRoute::new(self.0.into(), self.1)
    }
}

impl<S> IntoToolRoute<S, ()> for ToolRoute<S>
where
    S: MaybeSend + 'static,
{
    fn into_tool_route(self) -> ToolRoute<S> {
        self
    }
}

#[expect(clippy::exhaustive_structs, reason = "intentionally exhaustive")]
pub struct ToolAttrGenerateFunctionAdapter;
impl<S, F> IntoToolRoute<S, ToolAttrGenerateFunctionAdapter> for F
where
    S: MaybeSend + 'static,
    F: Fn() -> ToolRoute<S>,
{
    fn into_tool_route(self) -> ToolRoute<S> {
        (self)()
    }
}

pub trait CallToolHandlerExt<S, A>: Sized
where
    Self: CallToolHandler<S, A> + MaybeSend + Clone + 'static,
{
    fn name(self, name: impl Into<Cow<'static, str>>) -> WithToolAttr<Self, S, A>;
}

impl<C, S, A> CallToolHandlerExt<S, A> for C
where
    C: CallToolHandler<S, A> + MaybeSend + Clone + 'static,
{
    fn name(self, name: impl Into<Cow<'static, str>>) -> WithToolAttr<Self, S, A> {
        WithToolAttr {
            attr: Tool::new(
                name.into(),
                "",
                schema_for_type::<crate::model::JsonObject>(),
            ),
            call: self,
            _marker: std::marker::PhantomData,
        }
    }
}

#[non_exhaustive]
pub struct WithToolAttr<C, S, A>
where
    C: CallToolHandler<S, A> + MaybeSend + Clone + 'static,
{
    pub attr: crate::model::Tool,
    pub call: C,
    pub _marker: std::marker::PhantomData<fn(S, A)>,
}

impl<C, S, A> IntoToolRoute<S, A> for WithToolAttr<C, S, A>
where
    C: CallToolHandler<S, A> + MaybeSend + Clone + 'static,
    S: MaybeSend + 'static,
{
    fn into_tool_route(self) -> ToolRoute<S> {
        ToolRoute::new(self.attr, self.call)
    }
}

impl<C, S, A> WithToolAttr<C, S, A>
where
    C: CallToolHandler<S, A> + MaybeSend + Clone + 'static,
{
    pub fn description(mut self, description: impl Into<Cow<'static, str>>) -> Self {
        self.attr.description = Some(description.into());
        self
    }
    pub fn parameters<T: JsonSchema + 'static>(mut self) -> Self {
        self.attr.input_schema = schema_for_type::<T>();
        self
    }
    pub fn parameters_value(mut self, schema: serde_json::Value) -> Self {
        self.attr.input_schema = crate::model::object(schema).into();
        self
    }
    pub fn annotation(mut self, annotation: impl Into<ToolAnnotations>) -> Self {
        self.attr.annotations = Some(annotation.into());
        self
    }
}
#[non_exhaustive]
pub struct ToolRouter<S> {
    #[allow(clippy::type_complexity)]
    pub map: std::collections::HashMap<Cow<'static, str>, ToolRoute<S>>,

    pub transparent_when_not_found: bool,

    disabled: std::collections::HashSet<Cow<'static, str>>,

    notifier: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl<S> std::fmt::Debug for ToolRouter<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRouter")
            .field("map", &self.map)
            .field(
                "transparent_when_not_found",
                &self.transparent_when_not_found,
            )
            .field("disabled", &self.disabled)
            .field("notifier", &self.notifier.as_ref().map(|_| "..."))
            .finish()
    }
}

impl<S> Default for ToolRouter<S> {
    fn default() -> Self {
        Self {
            map: std::collections::HashMap::new(),
            transparent_when_not_found: false,
            disabled: std::collections::HashSet::new(),
            notifier: None,
        }
    }
}

impl<S> Clone for ToolRouter<S> {
    fn clone(&self) -> Self {
        Self {
            map: self.map.clone(),
            transparent_when_not_found: self.transparent_when_not_found,
            disabled: self.disabled.clone(),
            notifier: self.notifier.clone(),
        }
    }
}

impl<S> IntoIterator for ToolRouter<S> {
    type Item = ToolRoute<S>;
    type IntoIter = std::collections::hash_map::IntoValues<Cow<'static, str>, ToolRoute<S>>;

    fn into_iter(self) -> Self::IntoIter {
        let mut map = self.map;
        for name in &self.disabled {
            map.remove(name);
        }
        map.into_values()
    }
}

impl<S> ToolRouter<S>
where
    S: MaybeSend + 'static,
{
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_route<R, A>(mut self, route: R) -> Self
    where
        R: IntoToolRoute<S, A>,
    {
        self.add_route(route.into_tool_route());
        self
    }

    /// Add a tool that implements [`SyncTool`]
    pub fn with_sync_tool<T>(self) -> Self
    where
        T: SyncTool<S> + 'static,
    {
        if T::input_schema().is_some() {
            self.with_route((
                tool_traits::tool_attribute::<T>(),
                tool_traits::sync_tool_wrapper::<S, T>,
            ))
        } else {
            self.with_route((
                tool_traits::tool_attribute::<T>(),
                tool_traits::sync_tool_wrapper_with_empty_params::<S, T>,
            ))
        }
    }

    /// Add a tool that implements [`AsyncTool`]
    pub fn with_async_tool<T>(self) -> Self
    where
        T: AsyncTool<S> + 'static,
    {
        if T::input_schema().is_some() {
            self.with_route((
                tool_traits::tool_attribute::<T>(),
                tool_traits::async_tool_wrapper::<S, T>,
            ))
        } else {
            self.with_route((
                tool_traits::tool_attribute::<T>(),
                tool_traits::async_tool_wrapper_with_empty_params::<S, T>,
            ))
        }
    }

    pub fn add_route(&mut self, item: ToolRoute<S>) {
        let new_name = &item.attr.name;
        validate_and_warn_tool_name(new_name);
        self.map.insert(new_name.clone(), item);
    }

    pub fn merge(&mut self, other: ToolRouter<S>) {
        self.disabled.extend(other.disabled);
        for item in other.map.into_values() {
            self.add_route(item);
        }
    }

    /// Remove a tool route from the router.
    ///
    /// The disabled state is **preserved**: if the name was in the disabled
    /// set, it stays there so that a future [`add_route`](Self::add_route)
    /// or [`merge`](Self::merge) with the same name will inherit the
    /// disabled state. To also clear the disabled marker, call
    /// [`enable_route`](Self::enable_route) afterwards.
    pub fn remove_route(&mut self, name: &str) {
        self.map.remove(name);
    }

    /// Returns `true` if the tool is registered **and** not currently
    /// disabled.
    pub fn has_route(&self, name: &str) -> bool {
        self.map.contains_key(name) && !self.disabled.contains(name)
    }

    /// Disable a tool by name. Hidden from `list_all`, `get`, rejected by
    /// `call`. Re-enable with [`enable_route`](Self::enable_route).
    ///
    /// Returns `true` if the name was newly added to the disabled set.
    /// The name is recorded even if no matching route exists yet, so routes
    /// added later will inherit the disabled state.
    pub fn disable_route(&mut self, name: impl Into<Cow<'static, str>>) -> bool {
        let name = name.into();
        let was_visible = self.map.contains_key(&name) && !self.disabled.contains(&name);
        let newly_disabled = self.disabled.insert(name.clone());
        if was_visible && newly_disabled {
            self.notify_if_visible(&name);
        }
        newly_disabled
    }

    /// Re-enable a previously disabled tool. Returns `true` if the name
    /// was in the disabled set.
    pub fn enable_route(&mut self, name: &str) -> bool {
        let removed = self.disabled.remove(name);
        if removed {
            self.notify_if_visible(name);
        }
        removed
    }

    /// Returns `true` if the tool exists in the router **and** is currently
    /// disabled. Returns `false` if the tool does not exist or if the name
    /// was pre-disabled without a matching route.
    pub fn is_disabled(&self, name: &str) -> bool {
        self.map.contains_key(name) && self.disabled.contains(name)
    }

    /// Builder-style variant of [`disable_route`](Self::disable_route).
    ///
    /// The name is recorded even if no matching route has been added yet,
    /// so it can be called before [`with_route`](Self::with_route) in a
    /// builder chain.
    pub fn with_disabled(mut self, name: impl Into<Cow<'static, str>>) -> Self {
        self.disabled.insert(name.into());
        self
    }

    /// Install a callback invoked when the visible tool list changes.
    pub fn set_notifier(&mut self, f: impl Fn() + Send + Sync + 'static) {
        self.notifier = Some(Arc::new(f));
    }

    pub fn clear_notifier(&mut self) {
        self.notifier = None;
    }

    /// Install a notifier that sends `notifications/tools/list_changed`
    /// via the given peer.
    pub fn bind_peer_notifier(&mut self, peer: &crate::service::Peer<crate::RoleServer>) {
        let peer = peer.clone();
        self.set_notifier(move || {
            let peer = peer.clone();
            tokio::spawn(async move {
                if let Err(e) = peer.notify_tool_list_changed().await {
                    tracing::warn!("failed to send tools/list_changed notification: {e}");
                }
            });
        });
    }

    /// Deferred notifier: no-op until the peer slot is filled.
    pub(crate) fn deferred_peer_notifier() -> (
        impl Fn() + Send + Sync + 'static,
        Arc<std::sync::OnceLock<crate::service::Peer<crate::RoleServer>>>,
    ) {
        let peer_slot =
            Arc::new(std::sync::OnceLock::<crate::service::Peer<crate::RoleServer>>::new());
        let slot_clone = peer_slot.clone();
        let notifier = move || {
            if let Some(peer) = slot_clone.get() {
                let peer = peer.clone();
                tokio::spawn(async move {
                    if let Err(e) = peer.notify_tool_list_changed().await {
                        tracing::warn!("failed to send tools/list_changed notification: {e}");
                    }
                });
            }
        };
        (notifier, peer_slot)
    }

    fn notify_if_visible(&self, name: &str) {
        if self.map.contains_key(name) {
            if let Some(notifier) = &self.notifier {
                (notifier)();
            }
        }
    }

    pub async fn call(
        &self,
        context: ToolCallContext<'_, S>,
    ) -> Result<CallToolResult, crate::ErrorData> {
        let name = context.name();
        if self.disabled.contains(name) {
            return Err(crate::ErrorData::invalid_params("tool not found", None));
        }
        let item = self
            .map
            .get(name)
            .ok_or_else(|| crate::ErrorData::invalid_params("tool not found", None))?;

        let result = (item.call)(context).await?;

        Ok(result)
    }

    pub fn list_all(&self) -> Vec<crate::model::Tool> {
        let mut tools: Vec<_> = self
            .map
            .values()
            .filter(|item| !self.disabled.contains(&item.attr.name))
            .map(|item| item.attr.clone())
            .collect();
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        tools
    }

    /// Get a tool definition by name.
    ///
    /// Returns the tool if found and enabled, or `None` if the tool does not
    /// exist or is disabled.
    pub fn get(&self, name: &str) -> Option<&crate::model::Tool> {
        if self.disabled.contains(name) {
            return None;
        }
        self.map.get(name).map(|r| &r.attr)
    }
}

impl<S> std::ops::Add<ToolRouter<S>> for ToolRouter<S>
where
    S: MaybeSend + 'static,
{
    type Output = Self;

    fn add(mut self, other: ToolRouter<S>) -> Self::Output {
        self.merge(other);
        self
    }
}

impl<S> std::ops::AddAssign<ToolRouter<S>> for ToolRouter<S>
where
    S: MaybeSend + 'static,
{
    fn add_assign(&mut self, other: ToolRouter<S>) {
        self.merge(other);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{
        RoleServer,
        model::{CallToolRequestParams, ErrorCode, NumberOrString},
        service::{AtomicU32RequestIdProvider, Peer, RequestContext},
    };

    struct DummyService;
    impl crate::handler::server::ServerHandler for DummyService {}

    #[tokio::test]
    async fn test_call_disabled_tool_returns_error() {
        let service = DummyService;
        let mut router = ToolRouter::new().with_route(ToolRoute::new_dyn(
            crate::model::Tool::new("test_tool", "a test tool", Arc::new(Default::default())),
            |_ctx| Box::pin(async { Ok(CallToolResult::default()) }),
        ));
        router.disable_route("test_tool");

        let id_provider: Arc<dyn crate::service::RequestIdProvider> =
            Arc::new(AtomicU32RequestIdProvider::default());
        let (peer, _rx) = Peer::<RoleServer>::new(id_provider, None);
        let ctx = crate::handler::server::tool::ToolCallContext::new(
            &service,
            CallToolRequestParams {
                meta: None,
                name: Cow::Borrowed("test_tool"),
                arguments: None,
                task: None,
            },
            RequestContext::new(NumberOrString::Number(1), peer),
        );

        let err = router
            .call(ctx)
            .await
            .expect_err("disabled tool should reject");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert_eq!(err.message, "tool not found");
    }
}
