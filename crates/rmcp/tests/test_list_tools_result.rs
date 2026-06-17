#![cfg(all(feature = "server", feature = "macros", not(feature = "local")))]

use rmcp::{
    Json,
    handler::server::wrapper::Parameters,
    model::{ListToolsResult, NumberOrString, ServerJsonRpcMessage, ServerResult},
};

/// Parameters for adding two numbers.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct AddRequest {
    /// The left-hand number.
    a: f64,
    /// The right-hand number.
    b: f64,
}

/// Result of adding two numbers.
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
struct AddResult {
    /// The sum of the two numbers.
    sum: f64,
}

/// Add two numbers.
#[rmcp::tool]
fn add(Parameters(AddRequest { a, b }): Parameters<AddRequest>) -> Json<AddResult> {
    Json(AddResult { sum: a + b })
}

#[test]
fn list_tools_result_matches_expected_json() {
    let expected_json = std::fs::read("tests/test_list_tools_result/list_tools_result.json")
        .expect("missing expected list tools result JSON fixture");
    let expected: serde_json::Value =
        serde_json::from_slice(&expected_json).expect("invalid expected JSON fixture");

    assert_eq!(add(Parameters(AddRequest { a: 1.0, b: 2.0 })).0.sum, 3.0);

    let result = ListToolsResult::with_all_items(vec![add_tool_attr()]);
    let response = ServerJsonRpcMessage::response(
        ServerResult::ListToolsResult(result),
        NumberOrString::Number(2),
    );

    let actual = serde_json::to_value(response).expect("failed to serialize list tools response");
    assert_eq!(actual, expected);
}
