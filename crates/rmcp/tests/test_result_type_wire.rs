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
    CallToolResult, CompleteResult, ContentBlock, EmptyResult, GetPromptResult, GetSkillResult,
    ListPromptsResult, ListResourceTemplatesResult, ListResourcesResult, ListSkillsResult,
    ListToolsResult, ReadResourceResult, ResultType, ServerResult, Skill, SkillFrontmatter,
    SkillResources,
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
fn server_result_should_not_treat_meta_only_result_as_call_tool_result() {
    // `_meta` may appear on any result, so on its own it says nothing about the
    // result being a tool call.
    let payload = json!({
        "_meta": {
            "example.com/replica": "r1",
        },
    });

    let result =
        serde_json::from_value::<ServerResult>(payload.clone()).expect("deserialize ServerResult");

    match result {
        ServerResult::CustomResult(result) => assert_eq!(result.0, payload),
        other => panic!("expected CustomResult, got {other:?}"),
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
fn strip_removes_cache_hints_from_every_cacheable_result() {
    let skill = Skill::new(
        "skill://demo/SKILL.md",
        SkillFrontmatter::new("demo", "Demo skill"),
        SkillResources::Files(vec![]),
    );
    let results = [
        ServerResult::ListToolsResult(ListToolsResult::default().with_ttl_ms(60_000)),
        ServerResult::ListPromptsResult(ListPromptsResult::default().with_ttl_ms(60_000)),
        ServerResult::ListResourcesResult(ListResourcesResult::default().with_ttl_ms(60_000)),
        ServerResult::ListResourceTemplatesResult(
            ListResourceTemplatesResult::default().with_ttl_ms(60_000),
        ),
        ServerResult::ReadResourceResult(ReadResourceResult::new(vec![]).with_ttl_ms(60_000)),
        ServerResult::ListSkillsResult(ListSkillsResult::default().with_ttl_ms(60_000)),
        ServerResult::GetSkillResult(GetSkillResult::new(skill).with_ttl_ms(60_000)),
    ];

    for mut result in results {
        result.fill_missing_cache_hints();
        result.strip_cache_hints_for_legacy_peer();

        let value = serde_json::to_value(&result).expect("serialize ServerResult");
        assert!(value.get("ttlMs").is_none(), "{value}");
        assert!(value.get("cacheScope").is_none(), "{value}");
    }
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
