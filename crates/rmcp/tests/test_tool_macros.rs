use std::sync::Arc;

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

// define generic service trait
pub trait DataService: Send + Sync + 'static {
    fn get_data(&self) -> String;
}

// mock service for test
#[derive(Clone)]
struct MockDataService;
impl DataService for MockDataService {
    fn get_data(&self) -> String {
        "mock data".to_string()
    }
}

// define generic server
#[derive(Debug, Clone)]
pub struct GenericServer<DS: DataService> {
    data_service: Arc<DS>,
}

#[tool(tool_box)]
impl<DS: DataService> GenericServer<DS> {
    pub fn new(data_service: DS) -> Self {
        Self {
            data_service: Arc::new(data_service),
        }
    }

    #[tool(description = "Get data from the service")]
    async fn get_data(&self) -> String {
        self.data_service.get_data()
    }
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

#[tokio::test]
async fn test_tool_macros_with_generics() {
    let mock_service = MockDataService;
    let server = GenericServer::new(mock_service);
    let _attr = GenericServer::<MockDataService>::get_data_tool_attr();
    let _get_data_call_fn = GenericServer::<MockDataService>::get_data_tool_call;
    let _get_data_fn = GenericServer::<MockDataService>::get_data;
    assert_eq!(server.get_data().await, "mock data");
}

impl GetWeatherRequest {}
