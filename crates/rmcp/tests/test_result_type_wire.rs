//! Wire-shape regression guards for the SEP-2322 `resultType` discriminator.
//!
//! These pin the behavior that keeps older/strict peers working:
//! - `EmptyResult` stays a bare `{}` (some peers strict-validate empty results
//!   and reject extra keys),
//! - ordinary results carry `resultType: "complete"` by default, and
//! - [`ServerResult::strip_result_type_for_legacy_peer`] removes the
//!   `"complete"` discriminator so legacy sessions keep their historical wire
//!   shape (round-tripping a legacy result never adds the field).

use rmcp::model::{
    CallToolResult, CompleteResult, ContentBlock, EmptyResult, GetPromptResult, ListToolsResult,
    ReadResourceResult, ResultType, ServerResult,
};
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

// Guards the convention the server handler relies on: every constructor and
// `Default` impl produces `Some(COMPLETE)`, so 2026-07-28 sessions always
// include the spec-required field unless a handler clears it explicitly.
#[test]
fn constructors_and_defaults_produce_complete_result_type() {
    let complete = Some(ResultType::COMPLETE);
    assert_eq!(CallToolResult::default().result_type, complete);
    assert_eq!(CallToolResult::success(vec![]).result_type, complete);
    assert_eq!(CallToolResult::error(vec![]).result_type, complete);
    assert_eq!(CallToolResult::structured(json!({})).result_type, complete);
    assert_eq!(
        CallToolResult::structured_error(json!({})).result_type,
        complete
    );
    assert_eq!(ListToolsResult::default().result_type, complete);
    assert_eq!(
        ListToolsResult::with_all_items(vec![]).result_type,
        complete
    );
    assert_eq!(ReadResourceResult::new(vec![]).result_type, complete);
    assert_eq!(GetPromptResult::default().result_type, complete);
    assert_eq!(GetPromptResult::new(vec![]).result_type, complete);
    assert_eq!(CompleteResult::default().result_type, complete);
}

#[test]
fn absent_result_type_deserializes_as_none() {
    let legacy = json!({
        "content": [],
        "isError": false,
    });

    let result: CallToolResult =
        serde_json::from_value(legacy).expect("deserialize legacy CallToolResult");

    assert_eq!(result.result_type, None);
}

#[test]
fn legacy_call_tool_result_round_trips_without_result_type() {
    let legacy = json!({
        "content": [],
        "isError": false,
    });

    let result: CallToolResult =
        serde_json::from_value(legacy.clone()).expect("deserialize legacy CallToolResult");
    let reserialized = serde_json::to_value(result).expect("serialize CallToolResult");

    assert_eq!(reserialized, legacy);
}

#[test]
fn call_tool_result_should_reject_non_complete_result_type() {
    let input_required = json!({
        "resultType": "input_required",
        "_meta": {
            "example.com/replica": "r1",
        },
    });

    assert!(serde_json::from_value::<CallToolResult>(input_required).is_err());
}

#[test]
fn server_result_should_preserve_input_required_result_when_meta_present() {
    let input_required = json!({
        "resultType": "input_required",
        "requestState": "sealed",
        "_meta": {
            "example.com/replica": "r1",
        },
    });

    let result =
        serde_json::from_value::<ServerResult>(input_required).expect("deserialize ServerResult");

    match result {
        ServerResult::InputRequiredResult(result) => {
            assert_eq!(
                (result.request_state.as_deref(), result.meta.is_some()),
                (Some("sealed"), true)
            );
        }
        _ => panic!("expected InputRequiredResult"),
    }
}

#[test]
fn server_result_should_accept_legacy_call_tool_result_when_only_meta_present() {
    let legacy = json!({
        "_meta": {
            "example.com/replica": "r1",
        },
    });

    let result =
        serde_json::from_value::<ServerResult>(legacy).expect("deserialize legacy CallToolResult");

    match result {
        ServerResult::CallToolResult(result) => {
            assert_eq!(
                (
                    result.result_type,
                    result.content.is_empty(),
                    result.meta.is_some()
                ),
                (None, true, true)
            );
        }
        _ => panic!("expected CallToolResult"),
    }
}

#[test]
fn strip_removes_complete_result_type() {
    let mut result =
        ServerResult::CallToolResult(CallToolResult::success(vec![ContentBlock::text("ok")]));
    result.strip_result_type_for_legacy_peer();

    let value = serde_json::to_value(result).expect("serialize ServerResult");
    assert_eq!(
        value,
        json!({
            "content": [{ "type": "text", "text": "ok" }],
            "isError": false,
        })
    );
}

#[test]
fn strip_removes_complete_result_type_from_paginated_result() {
    let mut result = ServerResult::ListToolsResult(ListToolsResult::default());
    result.strip_result_type_for_legacy_peer();

    let value = serde_json::to_value(result).expect("serialize ServerResult");
    assert_eq!(value, json!({ "tools": [] }));
}

#[test]
fn strip_preserves_custom_result_type() {
    let streaming: ResultType =
        serde_json::from_value(json!("streaming")).expect("deserialize ResultType");
    let mut tool_result = CallToolResult::success(vec![ContentBlock::text("ok")]);
    tool_result.result_type = Some(streaming);

    let mut result = ServerResult::CallToolResult(tool_result);
    result.strip_result_type_for_legacy_peer();

    let value = serde_json::to_value(result).expect("serialize ServerResult");
    assert_eq!(value["resultType"], "streaming");
}

#[test]
fn meta_only_tool_results_preserve_legacy_and_complete_wire_forms() {
    for result_type in [None, Some("complete")] {
        let mut raw = json!({"_meta": {"trace": "present"}});
        if let Some(result_type) = result_type {
            raw["resultType"] = json!(result_type);
        }
        let tool: CallToolResult =
            serde_json::from_value(raw.clone()).expect("meta-only tool result");
        assert!(tool.content.is_empty());
        assert_eq!(tool.meta, serde_json::from_value(raw["_meta"].clone()).ok());
        assert!(matches!(
            serde_json::from_value::<ServerResult>(raw).unwrap(),
            ServerResult::CallToolResult(_)
        ));
    }
}

#[test]
fn custom_fields_with_meta_preserve_the_complete_extension_response() {
    for result_type in [None, Some("complete")] {
        for extension in [
            json!({"skills": []}),
            json!({"extension": null}),
            json!({"extension": false}),
        ] {
            let mut raw = json!({"_meta": {"trace": "present"}});
            raw.as_object_mut()
                .unwrap()
                .extend(extension.as_object().unwrap().clone());
            if let Some(result_type) = result_type {
                raw["resultType"] = json!(result_type);
            }
            let result: ServerResult =
                serde_json::from_value(raw.clone()).expect("extension result");
            match result {
                ServerResult::CustomResult(result) => assert_eq!(result.0, raw),
                other => panic!("expected lossless custom result, got {other:?}"),
            }
            assert!(serde_json::from_value::<CallToolResult>(raw).is_err());
        }
    }
}

#[test]
fn tool_specific_fields_keep_tool_results_with_unknown_extensions() {
    for tool_field in [
        json!({"content": []}),
        json!({"structuredContent": null}),
        json!({"isError": false}),
    ] {
        let mut raw =
            json!({"resultType": "complete", "_meta": {"trace": "present"}, "extension": null});
        raw.as_object_mut()
            .unwrap()
            .extend(tool_field.as_object().unwrap().clone());
        let result: ServerResult = serde_json::from_value(raw).expect("tool result");
        assert!(matches!(result, ServerResult::CallToolResult(_)));
    }
}
