use rmcp::{ServerHandler, handler::server::tool::ToolCallContext, tool};
use schemars::JsonSchema;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct GetWeatherRequest {
    pub city: String,
    pub date: String,
}

impl ServerHandler for Server {
    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParam,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::CallToolResult, rmcp::Error> {
        let tcc = ToolCallContext::new(self, request, context);
        match tcc.name() {
            "get-weather" => Self::get_weather_tool_call(tcc).await,
            _ => Err(rmcp::Error::invalid_params("method not found", None)),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Server {}

impl Server {
    /// This tool is used to get the weather of a city.
    #[tool(name = "get-weather", description = "Get the weather of a city.", vis = )]
    pub async fn get_weather(&self, #[tool(param)] city: String) -> String {
        drop(city);
        "rain".to_string()
    }
    #[tool(description = "Empty Parameter")]
    async fn empty_param(&self) {}
}

#[tokio::test]
async fn test_tool_macros() {
    let server = Server::default();
    let _attr = Server::get_weather_tool_attr();
    let _get_weather_call_fn = Server::get_weather_tool_call;
    let _get_weather_fn = Server::get_weather;
    server.get_weather("harbin".into()).await;
}

#[tokio::test]
async fn test_tool_macros_with_empty_param() {
    let _attr = Server::empty_param_tool_attr();
    println!("{_attr:?}");
    assert_eq!(_attr.input_schema.get("type").unwrap(), "object");
    assert!(_attr.input_schema.get("properties").is_none());
}

impl GetWeatherRequest {}
