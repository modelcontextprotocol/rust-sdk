#![cfg(not(feature = "local"))]
//! Test tool macros, including documentation for generated fns.

//cargo test --test test_tool_macros --features "client server"
// Enforce that all generated code has sufficient docs to pass missing_docs lint
#![deny(missing_docs)]
#![allow(dead_code)]
use std::sync::Arc;

use rmcp::{
    ClientHandler, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolRequestParams, ClientInfo, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Parameters for weather tool.
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct GetWeatherRequest {
    /// City of interest.
    pub city: String,
    /// Date of interest.
    pub date: String,
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Server {}

/// Trivial stateless server.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Server {
    tool_router: ToolRouter<Self>,
}

impl Server {
    /// Create weather server.
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router(router = tool_router)]
impl Server {
    /// This tool is used to get the weather of a city.
    #[tool(name = "get-weather", description = "Get the weather of a city.")]
    pub async fn get_weather(&self, city: Parameters<GetWeatherRequest>) -> String {
        drop(city);
        "rain".to_string()
    }

    #[tool]
    async fn empty_param(&self) {}
}

/// Generic service trait.
pub trait DataService: Send + Sync + 'static {
    /// Get data from service.
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

/// Generic server.
#[derive(Debug, Clone)]
pub struct GenericServer<DS: DataService> {
    data_service: Arc<DS>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl<DS: DataService> GenericServer<DS> {
    /// Create data server instance.
    pub fn new(data_service: DS) -> Self {
        Self {
            data_service: Arc::new(data_service),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Get data from the service")]
    async fn get_data(&self) -> String {
        self.data_service.get_data()
    }
}

#[tool_handler]
impl<DS: DataService> ServerHandler for GenericServer<DS> {}

#[tokio::test]
async fn test_tool_macros() {
    let server = Server::new();
    let _attr = Server::get_weather_tool_attr();
    let _get_weather_tool_attr_fn = Server::get_weather_tool_attr;
    let _get_weather_fn = Server::get_weather;
    server
        .get_weather(Parameters(GetWeatherRequest {
            city: "Harbin".into(),
            date: "Yesterday".into(),
        }))
        .await;
}

#[tokio::test]
async fn test_tool_macros_with_empty_param() {
    let _attr = Server::empty_param_tool_attr();
    println!("{_attr:?}");
    assert_eq!(
        _attr.input_schema.get("type"),
        Some(&serde_json::Value::String("object".to_string()))
    );
    assert_eq!(
        _attr.input_schema.get("properties"),
        Some(&serde_json::Value::Object(serde_json::Map::new()))
    );
}

#[tokio::test]
async fn test_tool_macros_with_generics() {
    let mock_service = MockDataService;
    let server = GenericServer::new(mock_service);
    let _attr = GenericServer::<MockDataService>::get_data_tool_attr();
    let _get_data_call_fn = GenericServer::<MockDataService>::get_data;
    let _get_data_fn = GenericServer::<MockDataService>::get_data;
    assert_eq!(server.get_data().await, "mock data");
}

#[tokio::test]
async fn test_tool_macros_with_optional_param() {
    let _attr = Server::get_weather_tool_attr();
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

/// Struct defined for testing optional field schema generation.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct OptionalFieldTestSchema {
    /// Field description.
    #[schemars(description = "An optional description field")]
    pub description: Option<String>,
}

/// Struct defined for testing optional i64 field schema generation and null handling.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct OptionalI64TestSchema {
    /// Optional count field.
    #[schemars(description = "An optional i64 field")]
    pub count: Option<i64>,

    /// Added to ensure non-empty object schema.
    pub mandatory_field: String,
}

/// Dummy struct to host the test tool method.
#[derive(Debug, Clone)]
pub struct OptionalSchemaTester {
    tool_router: ToolRouter<Self>,
}

impl Default for OptionalSchemaTester {
    fn default() -> Self {
        Self::new()
    }
}

impl OptionalSchemaTester {
    /// Create instance of optional schema tester service.
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl OptionalSchemaTester {
    // Dummy tool function using the test schema as an aggregated parameter
    #[tool(description = "A tool to test optional schema generation")]
    async fn test_optional(&self, _req: Parameters<OptionalFieldTestSchema>) {
        // Implementation doesn't matter for schema testing
        // Return type changed to () to satisfy IntoCallToolResult
    }

    // Tool function to test optional i64 handling
    #[tool(description = "A tool to test optional i64 schema generation")]
    async fn test_optional_i64(
        &self,
        Parameters(req): Parameters<OptionalI64TestSchema>,
    ) -> String {
        match req.count {
            Some(c) => format!("Received count: {}", c),
            None => "Received null count".to_string(),
        }
    }
}
#[tool_handler]
// Implement ServerHandler to route tool calls for OptionalSchemaTester
impl ServerHandler for OptionalSchemaTester {}

#[test]
fn test_optional_field_schema_generation_via_macro() {
    // tests https://github.com/modelcontextprotocol/rust-sdk/issues/135

    // Get the attributes generated by the #[tool] macro helper
    let tool_attr = OptionalSchemaTester::test_optional_tool_attr();

    // Print the actual generated schema for debugging
    println!(
        "Actual input schema generated by macro: {:#?}",
        tool_attr.input_schema
    );

    // Verify the schema generated for the aggregated OptionalFieldTestSchema
    // by the macro infrastructure using JSON Schema 2020-12 settings.
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

    // Assert nullable Option<T> is represented in JSON Schema 2020-12 form.
    let type_value = description_schema
        .get("type")
        .expect("Schema for Option<String> should include a type field");
    let type_array = type_value
        .as_array()
        .expect("Schema for Option<String> should use a type array [T, null]");
    assert_eq!(
        type_array,
        &vec![serde_json::json!("string"), serde_json::json!("null")],
        "Schema for Option<String> should be type: [\"string\", \"null\"]"
    );
    assert!(
        description_schema.get("nullable").is_none(),
        "Schema for Option<String> should not use OpenAPI nullable in JSON Schema 2020-12"
    );
    // We still check the description is correct
    assert_eq!(
        description_schema
            .get("description")
            .map(|v| v.as_str().unwrap()),
        Some("An optional description field")
    );

    // Ensure no OpenAPI-only nullable extension was emitted.
    assert!(description_schema.get("nullable").is_none());
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
    let server = OptionalSchemaTester::new();
    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    // Create a simple client handler that just forwards tool calls
    let client_handler = DummyClientHandler::default();
    let client = client_handler.serve(client_transport).await?;

    // Test null case
    let result = client
        .call_tool(
            CallToolRequestParams::new("test_optional_i64").with_arguments(
                serde_json::json!({
                    "count": null,
                    "mandatory_field": "test_null"
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
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
        .call_tool(
            CallToolRequestParams::new("test_optional_i64").with_arguments(
                serde_json::json!({
                    "count": 42,
                    "mandatory_field": "test_some"
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
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

// --- Tests for field-free minimal server pattern (issue #711) ---

/// Minimal server: no tool_router field, no new(), no get_info().
#[derive(Debug, Clone)]
pub struct MinimalServer;

#[tool_router]
impl MinimalServer {
    #[tool(description = "Say hello")]
    fn hello(&self) -> String {
        "hello".to_string()
    }
}

#[tool_handler]
impl ServerHandler for MinimalServer {}

#[test]
fn test_minimal_server_get_info_auto_generated() {
    let server = MinimalServer;
    let info = server.get_info();

    assert!(
        info.capabilities.tools.is_some(),
        "tools capability should be enabled"
    );
    assert!(
        info.capabilities.prompts.is_none(),
        "prompts should not be auto-enabled"
    );
    assert!(
        info.capabilities.tasks.is_none(),
        "tasks should not be auto-enabled"
    );
    assert!(
        !info.server_info.name.is_empty(),
        "server name should not be empty"
    );
    assert!(
        !info.server_info.version.is_empty(),
        "server version should not be empty"
    );
    assert!(
        info.instructions.is_none(),
        "instructions should be None by default"
    );
}

#[tokio::test]
async fn test_minimal_server_tool_call() -> anyhow::Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(4096);

    let server_handle = tokio::spawn(async move {
        MinimalServer
            .serve(server_transport)
            .await?
            .waiting()
            .await?;
        anyhow::Ok(())
    });

    let client = DummyClientHandler::default()
        .serve(client_transport)
        .await?;

    let result = client
        .call_tool(CallToolRequestParams::new("hello"))
        .await?;

    let text = result
        .content
        .first()
        .and_then(|c| c.raw.as_text())
        .map(|t| t.text.as_str())
        .expect("Expected text content");

    assert_eq!(text, "hello");

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

/// Server with custom name/version/instructions via tool_handler attributes.
#[derive(Debug, Clone)]
pub struct CustomInfoServer;

#[tool_router]
impl CustomInfoServer {
    #[tool(description = "Ping")]
    fn ping(&self) -> String {
        "pong".to_string()
    }
}

#[tool_handler(
    name = "my-custom-server",
    version = "2.0.0",
    instructions = "A custom server"
)]
impl ServerHandler for CustomInfoServer {}

#[test]
fn test_custom_info_server() {
    let server = CustomInfoServer;
    let info = server.get_info();

    assert_eq!(info.server_info.name, "my-custom-server");
    assert_eq!(info.server_info.version, "2.0.0");
    assert_eq!(info.instructions.as_deref(), Some("A custom server"));
    assert!(info.capabilities.tools.is_some());
}

/// Server that provides its own get_info() — macro should not override it.
#[derive(Debug, Clone)]
pub struct ManualInfoServer;

#[tool_router]
impl ManualInfoServer {
    #[tool(description = "Noop")]
    fn noop(&self) {}
}

#[tool_handler]
impl ServerHandler for ManualInfoServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(rmcp::model::Implementation::new("manual", "9.9.9"))
    }
}

#[test]
fn test_manual_get_info_not_overridden() {
    let server = ManualInfoServer;
    let info = server.get_info();

    assert_eq!(info.server_info.name, "manual");
    assert_eq!(info.server_info.version, "9.9.9");
    assert!(info.capabilities.tools.is_some());
    assert!(
        info.capabilities.resources.is_some(),
        "manual resources should be preserved"
    );
}
