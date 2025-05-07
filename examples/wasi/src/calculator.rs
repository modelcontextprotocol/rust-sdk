use rmcp::{
    ServerHandler,
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_box,
};

#[derive(Debug, rmcp::serde::Deserialize, schemars::JsonSchema)]
pub struct SumRequest {
    #[schemars(description = "the left hand side number")]
    pub a: i32,
    pub b: i32,
}
#[derive(Debug, Clone)]
pub struct Calculator;
impl Calculator {
    #[tool(description = "Calculate the sum of two numbers",aggr)]
    fn sum(&self, SumRequest { a, b }: SumRequest) -> String {
        (a + b).to_string()
    }

    #[tool(description = "Calculate the sub of two numbers")]
    fn sub(
        &self,
        #[schemars(description = "the left hand side number")]
        a: i32,
        #[schemars(description = "the right hand side number")]
        b: i32,
    ) -> String {
        (a - b).to_string()
    }

    tool_box!(Calculator { sum, sub });
}

impl ServerHandler for Calculator {
    tool_box!(@derive);
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some("A simple calculator".into()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}
