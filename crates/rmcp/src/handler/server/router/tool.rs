use std::any::{Any, TypeId};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use futures::FutureExt;
use futures::future::BoxFuture;
use schemars::JsonSchema;

use crate::model::{CallToolResult, Tool, ToolAnnotations};

use crate::handler::server::tool::{
    CallToolHandler, DynCallToolHandler, ToolCallContext, schema_for_type,
};

inventory::collect!(ToolRouteWithType);

#[derive(Debug, Default)]
pub struct GlobalStaticRouters {
    pub routers:
        std::sync::OnceLock<tokio::sync::RwLock<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>>,
}

impl GlobalStaticRouters {
    pub fn global() -> &'static Self {
        static GLOBAL: GlobalStaticRouters = GlobalStaticRouters {
            routers: std::sync::OnceLock::new(),
        };
        &GLOBAL
    }
    pub async fn set<S: Send + Sync + 'static>(router: Arc<ToolRouter<S>>) -> Result<(), String> {
        let routers = Self::global().routers.get_or_init(Default::default);
        let mut routers_wg = routers.write().await;
        if routers_wg.insert(TypeId::of::<S>(), router).is_some() {
            return Err("Router already exists".to_string());
        }
        Ok(())
    }
    pub async fn get<S: Send + Sync + 'static>() -> Arc<ToolRouter<S>> {
        let routers = Self::global().routers.get_or_init(Default::default);
        let routers_rg = routers.read().await;
        if let Some(router) = routers_rg.get(&TypeId::of::<S>()) {
            return router
                .clone()
                .downcast::<ToolRouter<S>>()
                .expect("Failed to downcast");
        }
        {
            drop(routers_rg);
        }
        let mut routers = routers.write().await;
        match routers.entry(TypeId::of::<S>()) {
            std::collections::hash_map::Entry::Occupied(occupied) => occupied
                .get()
                .clone()
                .downcast::<ToolRouter<S>>()
                .expect("Failed to downcast"),
            std::collections::hash_map::Entry::Vacant(vacant) => {
                let mut router = ToolRouter::<S>::default();
                for route in inventory::iter::<ToolRouteWithType>
                    .into_iter()
                    .filter(|r| r.type_id == TypeId::of::<S>())
                {
                    if let Some(route) = route.downcast::<S>() {
                        router.add_route(route.clone());
                    }
                }
                let mut_ref = vacant.insert(Arc::new(router));
                mut_ref
                    .downcast_ref()
                    .cloned()
                    .expect("Failed to downcast after insert")
            }
        }
    }
}

pub struct ToolRouteWithType {
    type_id: TypeId,
    route: Box<dyn Any + Send + Sync>,
}

impl ToolRouteWithType {
    pub fn downcast<S: 'static>(&self) -> Option<&ToolRoute<S>> {
        if self.type_id == TypeId::of::<S>() {
            self.route.downcast_ref::<ToolRoute<S>>()
        } else {
            None
        }
    }
    pub fn from_tool_route<S: 'static>(route: ToolRoute<S>) -> Self {
        Self {
            type_id: TypeId::of::<S>(),
            route: Box::new(route),
        }
    }
}

impl<S: 'static> From<ToolRoute<S>> for ToolRouteWithType {
    fn from(value: ToolRoute<S>) -> Self {
        Self::from_tool_route(value)
    }
}

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

impl<S: Send + Sync + 'static> ToolRoute<S> {
    pub fn new<C, A>(attr: impl Into<Tool>, call: C) -> Self
    where
        C: CallToolHandler<S, A> + Send + Sync + Clone + 'static,
    {
        Self {
            call: Arc::new(move |context: ToolCallContext<S>| {
                let call = call.clone();
                context.invoke(call).boxed()
            }),
            attr: attr.into(),
        }
    }
    pub fn new_dyn<C>(attr: impl Into<Tool>, call: C) -> Self
    where
        C: for<'a> Fn(
                ToolCallContext<'a, S>,
            ) -> BoxFuture<'a, Result<CallToolResult, crate::Error>>
            + Send
            + Sync
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
    S: Send + Sync + 'static,
    C: CallToolHandler<S, A> + Send + Sync + Clone + 'static,
    T: Into<Tool>,
{
    fn into_tool_route(self) -> ToolRoute<S> {
        ToolRoute::new(self.0.into(), self.1)
    }
}

impl<S> IntoToolRoute<S, ()> for ToolRoute<S>
where
    S: Send + Sync + 'static,
{
    fn into_tool_route(self) -> ToolRoute<S> {
        self
    }
}

pub struct ToolAttrGenerateFunctionAdapter;
impl<S, F> IntoToolRoute<S, ToolAttrGenerateFunctionAdapter> for F
where
    S: Send + Sync + 'static,
    F: Fn() -> ToolRoute<S>,
{
    fn into_tool_route(self) -> ToolRoute<S> {
        (self)()
    }
}

pub trait CallToolHandlerExt<S, A>: Sized
where
    Self: CallToolHandler<S, A> + Send + Sync + Clone + 'static,
{
    fn name(self, name: impl Into<Cow<'static, str>>) -> WithToolAttr<Self, S, A>;
}

impl<C, S, A> CallToolHandlerExt<S, A> for C
where
    C: CallToolHandler<S, A> + Send + Sync + Clone + 'static,
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

pub struct WithToolAttr<C, S, A>
where
    C: CallToolHandler<S, A> + Send + Sync + Clone + 'static,
{
    pub attr: crate::model::Tool,
    pub call: C,
    pub _marker: std::marker::PhantomData<fn(S, A)>,
}

impl<C, S, A> IntoToolRoute<S, A> for WithToolAttr<C, S, A>
where
    C: CallToolHandler<S, A> + Send + Sync + Clone + 'static,
    S: Send + Sync + 'static,
{
    fn into_tool_route(self) -> ToolRoute<S> {
        ToolRoute::new(self.attr, self.call)
    }
}

impl<C, S, A> WithToolAttr<C, S, A>
where
    C: CallToolHandler<S, A> + Send + Sync + Clone + 'static,
{
    pub fn description(mut self, description: impl Into<Cow<'static, str>>) -> Self {
        self.attr.description = Some(description.into());
        self
    }
    pub fn parameters<T: JsonSchema>(mut self) -> Self {
        self.attr.input_schema = schema_for_type::<T>().into();
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
#[derive(Debug)]
pub struct ToolRouter<S> {
    #[allow(clippy::type_complexity)]
    pub map: std::collections::HashMap<Cow<'static, str>, ToolRoute<S>>,

    pub transparent_when_not_found: bool,
}

impl<S> Default for ToolRouter<S> {
    fn default() -> Self {
        Self {
            map: std::collections::HashMap::new(),
            transparent_when_not_found: false,
        }
    }
}
impl<S> Clone for ToolRouter<S> {
    fn clone(&self) -> Self {
        Self {
            map: self.map.clone(),
            transparent_when_not_found: self.transparent_when_not_found,
        }
    }
}

impl<S> IntoIterator for ToolRouter<S> {
    type Item = ToolRoute<S>;
    type IntoIter = std::collections::hash_map::IntoValues<Cow<'static, str>, ToolRoute<S>>;

    fn into_iter(self) -> Self::IntoIter {
        self.map.into_values()
    }
}

impl<S> ToolRouter<S>
where
    S: Send + Sync + 'static,
{
    pub fn new() -> Self {
        Self {
            map: std::collections::HashMap::new(),
            transparent_when_not_found: false,
        }
    }
    pub fn with_route<C, A>(mut self, attr: crate::model::Tool, call: C) -> Self
    where
        C: CallToolHandler<S, A> + Send + Sync + Clone + 'static,
    {
        self.add_route(ToolRoute::new(attr, call));
        self
    }

    pub fn add_route(&mut self, item: ToolRoute<S>) {
        self.map.insert(item.attr.name.clone(), item);
    }

    pub fn merge(&mut self, other: ToolRouter<S>) {
        for item in other.map.into_values() {
            self.add_route(item);
        }
    }

    pub fn remove_route<H, A>(&mut self, name: &str) {
        self.map.remove(name);
    }
    pub fn has_route(&self, name: &str) -> bool {
        self.map.contains_key(name)
    }
    pub async fn call(
        &self,
        context: ToolCallContext<'_, S>,
    ) -> Result<CallToolResult, crate::Error> {
        let item = self
            .map
            .get(context.name())
            .ok_or_else(|| crate::Error::invalid_params("tool not found", None))?;
        (item.call)(context).await
    }

    pub fn list_all(&self) -> Vec<crate::model::Tool> {
        self.map.values().map(|item| item.attr.clone()).collect()
    }
}

impl<S> std::ops::Add<ToolRouter<S>> for ToolRouter<S>
where
    S: Send + Sync + 'static,
{
    type Output = Self;

    fn add(mut self, other: ToolRouter<S>) -> Self::Output {
        self.merge(other);
        self
    }
}

impl<S> std::ops::AddAssign<ToolRouter<S>> for ToolRouter<S>
where
    S: Send + Sync + 'static,
{
    fn add_assign(&mut self, other: ToolRouter<S>) {
        self.merge(other);
    }
}
