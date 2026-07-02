//! Wire-shape regression guards for the SEP-2322 `resultType` discriminator.
//!
//! These pin the behavior that keeps older/strict peers working:
//! - `EmptyResult` stays a bare `{}` (some peers strict-validate empty results
//!   and reject extra keys), and
//! - ordinary results carry `resultType: "complete"`.

use rmcp::model::{CallToolResult, ContentBlock, EmptyResult, ListToolsResult};
use serde_json::json;

#[test]
fn empty_result_serializes_without_result_type() {
    let value = serde_json::to_value(EmptyResult {}).expect("serialize EmptyResult");
    assert_eq!(value, json!({}));
}

#[test]
fn call_tool_result_serializes_complete_result_type() {
    let value = serde_json::to_value(CallToolResult::success(vec![ContentBlock::text("ok")]))
        .expect("serialize CallToolResult");
    assert_eq!(value["resultType"], "complete");
}

#[test]
fn paginated_result_serializes_complete_result_type() {
    let value =
        serde_json::to_value(ListToolsResult::default()).expect("serialize ListToolsResult");
    assert_eq!(value["resultType"], "complete");
}
