use rmcp::{
    ServerHandler,
    model::{ServerCapabilities, ServerInfo},
    schemars, tool,
};
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SumRequest {
    #[schemars(description = "the left hand side number")]
    pub a: i32,
    pub b: i32,
}
#[derive(Debug, Clone, Default)]
pub struct Calculator;
#[tool(tool_box,description = "A simple calculator")]
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
}
