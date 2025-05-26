//cargo test --test test_tool_macros --features "client server"

use std::sync::Arc;

use rmcp::{
    ClientHandler, ServerHandler, ServiceExt,
    handler::server::tool::ToolCallContext,
    model::{CallToolRequestParam, ClientInfo},
    tool,
};
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

    #[tool(description = "Optional Parameter")]
    async fn optional_param(&self, #[tool(param)] city: Option<String>) -> String {
        city.unwrap_or_default()
    }
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
#[tool(tool_box)]
impl<DS: DataService> ServerHandler for GenericServer<DS> {}

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

#[tokio::test]
async fn test_tool_macros_with_optional_param() {
    let _attr = Server::optional_param_tool_attr();
    // println!("{_attr:?}");
    let attr_type = _attr
        .input_schema
        .get("properties")
        .unwrap()
        .get("city")
        .unwrap()
        .get("type")
        .unwrap();
    println!("_attr.input_schema: {:?}", attr_type);
    assert_eq!(attr_type.as_str().unwrap(), "string");
}

impl GetWeatherRequest {}

// Struct defined for testing optional field schema generation
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct OptionalFieldTestSchema {
    #[schemars(description = "An optional description field")]
    pub description: Option<String>,
}

// Struct defined for testing optional i64 field schema generation and null handling
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct OptionalI64TestSchema {
    #[schemars(description = "An optional i64 field")]
    pub count: Option<i64>,
    pub mandatory_field: String, // Added to ensure non-empty object schema
}

// Dummy struct to host the test tool method
#[derive(Debug, Clone, Default)]
pub struct OptionalSchemaTester {}

impl OptionalSchemaTester {
    // Dummy tool function using the test schema as an aggregated parameter
    #[tool(description = "A tool to test optional schema generation")]
    async fn test_optional_aggr(&self, #[tool(aggr)] _req: OptionalFieldTestSchema) {
        // Implementation doesn't matter for schema testing
        // Return type changed to () to satisfy IntoCallToolResult
    }

    // Tool function to test optional i64 handling
    #[tool(description = "A tool to test optional i64 schema generation")]
    async fn test_optional_i64_aggr(&self, #[tool(aggr)] req: OptionalI64TestSchema) -> String {
        match req.count {
            Some(c) => format!("Received count: {}", c),
            None => "Received null count".to_string(),
        }
    }
}

// Implement ServerHandler to route tool calls for OptionalSchemaTester
impl ServerHandler for OptionalSchemaTester {
    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParam,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::CallToolResult, rmcp::Error> {
        let tcc = ToolCallContext::new(self, request, context);
        match tcc.name() {
            "test_optional_aggr" => Self::test_optional_aggr_tool_call(tcc).await,
            "test_optional_i64_aggr" => Self::test_optional_i64_aggr_tool_call(tcc).await,
            _ => Err(rmcp::Error::invalid_params("method not found", None)),
        }
    }
}

#[test]
fn test_optional_field_schema_generation_via_macro() {
    // tests https://github.com/modelcontextprotocol/rust-sdk/issues/135

    // Get the attributes generated by the #[tool] macro helper
    let tool_attr = OptionalSchemaTester::test_optional_aggr_tool_attr();

    // Print the actual generated schema for debugging
    println!(
        "Actual input schema generated by macro: {:#?}",
        tool_attr.input_schema
    );

    // Verify the schema generated for the aggregated OptionalFieldTestSchema
    // by the macro infrastructure (which should now use OpenAPI 3 settings)
    let input_schema_map = &*tool_attr.input_schema; // Dereference Arc<JsonObject>

    // Check the schema for the 'description' property within the input schema
    let properties = input_schema_map
        .get("properties")
        .expect("Schema should have properties")
        .as_object()
        .unwrap();
    let description_schema = properties
        .get("description")
        .expect("Properties should include description")
        .as_object()
        .unwrap();

    // Assert that the format is now `type: "string", nullable: true`
    assert_eq!(
        description_schema.get("type").map(|v| v.as_str().unwrap()),
        Some("string"),
        "Schema for Option<String> generated by macro should be type: \"string\""
    );
    assert_eq!(
        description_schema
            .get("nullable")
            .map(|v| v.as_bool().unwrap()),
        Some(true),
        "Schema for Option<String> generated by macro should have nullable: true"
    );
    // We still check the description is correct
    assert_eq!(
        description_schema
            .get("description")
            .map(|v| v.as_str().unwrap()),
        Some("An optional description field")
    );

    // Ensure the old 'type: [T, null]' format is NOT used
    let type_value = description_schema.get("type").unwrap();
    assert!(
        !type_value.is_array(),
        "Schema type should not be an array [T, null]"
    );
}

// Define a dummy client handler
#[derive(Debug, Clone, Default)]
struct DummyClientHandler {}

impl ClientHandler for DummyClientHandler {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::default()
    }
}

#[tokio::test]
async fn test_optional_i64_field_with_null_input() -> anyhow::Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(4096);

    // Server setup
    let server = OptionalSchemaTester::default();
    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    // Create a simple client handler that just forwards tool calls
    let client_handler = DummyClientHandler::default();
    let client = client_handler.serve(client_transport).await?;

    // Test null case
    let result = client
        .call_tool(CallToolRequestParam {
            name: "test_optional_i64_aggr".into(),
            arguments: Some(
                serde_json::json!({
                    "count": null,
                    "mandatory_field": "test_null"
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        })
        .await?;

    let result_text = result
        .content
        .first()
        .and_then(|content| content.raw.as_text())
        .map(|text| text.text.as_str())
        .expect("Expected text content");

    assert_eq!(
        result_text, "Received null count",
        "Null case should return expected message"
    );

    // Test Some case
    let some_result = client
        .call_tool(CallToolRequestParam {
            name: "test_optional_i64_aggr".into(),
            arguments: Some(
                serde_json::json!({
                    "count": 42,
                    "mandatory_field": "test_some"
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        })
        .await?;

    let some_result_text = some_result
        .content
        .first()
        .and_then(|content| content.raw.as_text())
        .map(|text| text.text.as_str())
        .expect("Expected text content");

    assert_eq!(
        some_result_text, "Received count: 42",
        "Some case should return expected message"
    );

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}
