use std::borrow::Cow;

use futures::future::BoxFuture;
use schemars::JsonSchema;

use crate::model::{CallToolResult, Tool, ToolAnnotations};

use crate::handler::server::tool::{
    CallToolHandler, DynCallToolHandler, ToolCallContext, schema_for_type,
};

pub struct ToolRoute<S> {
    #[allow(clippy::type_complexity)]
    pub call: Box<DynCallToolHandler<S>>,
    pub attr: crate::model::Tool,
}

impl<S: Send + Sync + 'static> ToolRoute<S> {
    pub fn new<C, A>(attr: impl Into<Tool>, call: C) -> Self
    where
        C: CallToolHandler<S, A> + Send + Sync + Clone + 'static,
        <C as CallToolHandler<S, A>>::Fut: 'static,
    {
        Self {
            call: Box::new(move |context: ToolCallContext<S>| {
                let call = call.clone();
                Box::pin(async move { context.invoke(call).await })
            }),
            attr: attr.into(),
        }
    }
    pub fn new_dyn<C>(attr: impl Into<Tool>, call: C) -> Self
    where
        C: Fn(ToolCallContext<S>) -> BoxFuture<'static, Result<CallToolResult, crate::Error>>
            + Send
            + Sync
            + 'static,
    {
        Self {
            call: Box::new(call),
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
    <C as CallToolHandler<S, A>>::Fut: 'static,
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
    <Self as CallToolHandler<S, A>>::Fut: 'static,
{
    fn name(self, name: impl Into<Cow<'static, str>>) -> WithToolAttr<Self, S, A>;
}

impl<C, S, A> CallToolHandlerExt<S, A> for C
where
    C: CallToolHandler<S, A> + Send + Sync + Clone + 'static,
    <C as CallToolHandler<S, A>>::Fut: 'static,
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
    <C as CallToolHandler<S, A>>::Fut: 'static,
{
    pub attr: crate::model::Tool,
    pub call: C,
    pub _marker: std::marker::PhantomData<fn(S, A)>,
}

impl<C, S, A> IntoToolRoute<S, A> for WithToolAttr<C, S, A>
where
    C: CallToolHandler<S, A> + Send + Sync + Clone + 'static,
    <C as CallToolHandler<S, A>>::Fut: 'static,
    S: Send + Sync + 'static,
{
    fn into_tool_route(self) -> ToolRoute<S> {
        ToolRoute::new(self.attr, self.call)
    }
}

impl<C, S, A> WithToolAttr<C, S, A>
where
    C: CallToolHandler<S, A> + Send + Sync + Clone + 'static,
    <C as CallToolHandler<S, A>>::Fut: 'static,
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

#[derive(Default)]
pub struct ToolRouter<S> {
    #[allow(clippy::type_complexity)]
    pub map: std::collections::HashMap<Cow<'static, str>, ToolRoute<S>>,

    pub transparent_when_not_found: bool,
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
    pub fn with<C, A>(mut self, attr: crate::model::Tool, call: C) -> Self
    where
        C: CallToolHandler<S, A> + Send + Sync + Clone + 'static,
        <C as CallToolHandler<S, A>>::Fut: 'static,
    {
        self.add(ToolRoute::new(attr, call));
        self
    }

    pub fn add(&mut self, item: ToolRoute<S>) {
        self.map.insert(item.attr.name.clone(), item);
    }

    pub fn remove<H, A>(&mut self, name: &str) {
        self.map.remove(name);
    }
    pub fn has(&self, name: &str) -> bool {
        self.map.contains_key(name)
    }
    pub async fn call(&self, context: ToolCallContext<S>) -> Result<CallToolResult, crate::Error> {
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
