use std::{collections::HashMap, sync::Arc};

use rmcp::{
    RoleServer, ServerHandler, Service,
    handler::server::{
        router::{
            Router,
            tool::{CallToolHandlerExt, ToolRoute, ToolRouter},
        },
        tool::{Parameters, schema_for_type},
    },
    model::{Extensions, Tool},
};

#[derive(Debug, Default)]
pub struct TestHandler<T: 'static = ()> {
    pub _marker: std::marker::PhantomData<fn(*const T)>,
}

impl<T: 'static> ServerHandler for TestHandler<T> {}
#[derive(Debug, schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
pub struct Request {
    pub fields: HashMap<String, String>,
}

#[derive(Debug, schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
pub struct Sum {
    pub a: i32,
    pub b: i32,
}

impl<T> TestHandler<T> {
    async fn async_method(self: Arc<Self>, Parameters(Request { fields }): Parameters<Request>) {
        drop(fields)
    }
    fn sync_method(&self, Parameters(Request { fields }): Parameters<Request>) {
        drop(fields)
    }
}

fn sync_function(Parameters(Request { fields }): Parameters<Request>) {
    drop(fields)
}

// #[rmcp(tool(description = "async method", parameters = Request, name = "async_method"))]
//    ^
//    |_____ this is a macro will generates a function with the same name but return ToolRoute<TestHandler>
fn async_function<T>(
    _callee: Arc<TestHandler<T>>,
    Parameters(Request { fields }): Parameters<Request>,
) {
    drop(fields)
}

fn attr_generator_fn<S: Send + Sync + 'static>() -> ToolRoute<S> {
    ToolRoute::new(
        Tool::new(
            "sync_method_from_generator_fn",
            "a sync method tool",
            schema_for_type::<Request>(),
        ),
        sync_function,
    )
}

fn assert_service<S: Service<RoleServer>>(service: S) {
    drop(service);
}

#[test]
fn test_tool_router() {
    let test_handler = TestHandler::<()>::default();
    fn tool(name: &'static str) -> Tool {
        Tool::new(name, name, schema_for_type::<Request>())
    }
    let tool_router = ToolRouter::<TestHandler<()>>::new()
        .with(tool("sync_method"), TestHandler::sync_method)
        .with(tool("async_method"), TestHandler::async_method)
        .with(tool("sync_function"), sync_function)
        .with(tool("async_function"), async_function);

    let router = Router::new(test_handler)
        .with_tool(
            TestHandler::sync_method
                .name("sync_method")
                .description("a sync method tool")
                .parameters::<Request>(),
        )
        .with_tool(
            (|Parameters(Sum { a, b }): Parameters<Sum>| (a + b).to_string())
                .name("add")
                .parameters::<Sum>(),
        )
        .with_tool(attr_generator_fn)
        .with_tools(tool_router);
    assert_service(router);
}
