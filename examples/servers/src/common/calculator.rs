use rmcp::{
    ServerHandler,
    handler::server::wrapper::Json,
    model::{ServerCapabilities, ServerInfo},
    schemars, tool,
};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SumRequest {
    #[schemars(description = "the left hand side number")]
    pub a: i32,
    pub b: i32,
}
#[derive(Debug, Clone)]
pub struct Calculator;
#[tool(tool_box)]
impl Calculator {
    #[tool(description = "Calculate the sum of two numbers")]
    fn sum(&self, #[tool(aggr)] SumRequest { a, b }: SumRequest) -> String {
        (a + b).to_string()
    }

    #[tool(description = "Calculate the sum of two numbers")]
    fn sub(
        &self,
        #[tool(param)]
        #[schemars(description = "the left hand side number")]
        a: i32,
        #[tool(param)]
        #[schemars(description = "the left hand side number")]
        b: i32,
    ) -> Json<i32> {
        Json(a - b)
    }
}

#[tool(tool_box)]
impl ServerHandler for Calculator {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some("A simple calculator".into()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}
