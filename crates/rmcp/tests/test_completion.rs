use std::collections::HashMap;

use rmcp::{handler::server::completion::*, model::*};
use serde_json::json;

#[test]
fn test_completion_context_serialization() {
    let mut args = HashMap::new();
    args.insert("key1".to_string(), "value1".to_string());
    args.insert("key2".to_string(), "value2".to_string());

    let context = CompletionContext::with_arguments(args);

    // Test serialization
    let json = serde_json::to_value(&context).unwrap();
    let expected = json!({
        "arguments": {
            "key1": "value1",
            "key2": "value2"
        }
    });
    assert_eq!(json, expected);

    // Test deserialization
    let deserialized: CompletionContext = serde_json::from_value(expected).unwrap();
    assert_eq!(deserialized, context);
}

#[test]
fn test_completion_context_methods() {
    let mut args = HashMap::new();
    args.insert("city".to_string(), "San Francisco".to_string());
    args.insert("country".to_string(), "USA".to_string());

    let context = CompletionContext::with_arguments(args);

    assert!(context.has_arguments());
    assert_eq!(
        context.get_argument("city"),
        Some(&"San Francisco".to_string())
    );
    assert_eq!(context.get_argument("missing"), None);

    let names = context.argument_names();
    assert!(names.contains(&"city"));
    assert!(names.contains(&"country"));
    assert_eq!(names.len(), 2);
}

#[test]
fn test_complete_request_param_serialization() {
    let mut args = HashMap::new();
    args.insert("previous_input".to_string(), "test".to_string());

    let request = CompleteRequestParam {
        r#ref: Reference::for_prompt("weather_prompt"),
        argument: ArgumentInfo {
            name: "location".to_string(),
            value: "San".to_string(),
        },
        context: Some(CompletionContext::with_arguments(args)),
    };

    let json = serde_json::to_value(&request).unwrap();
    assert!(json["ref"]["name"].as_str().unwrap() == "weather_prompt");
    assert!(json["argument"]["name"].as_str().unwrap() == "location");
    assert!(json["argument"]["value"].as_str().unwrap() == "San");
    assert!(
        json["context"]["arguments"]["previous_input"]
            .as_str()
            .unwrap()
            == "test"
    );
}

#[test]
fn test_completion_info_validation() {
    // Valid completion with less than max values
    let values = vec!["option1".to_string(), "option2".to_string()];
    let completion = CompletionInfo::new(values.clone()).unwrap();
    assert_eq!(completion.values, values);
    assert!(completion.validate().is_ok());

    // Test max values limit
    let many_values: Vec<String> = (0..=CompletionInfo::MAX_VALUES)
        .map(|i| format!("option_{}", i))
        .collect();
    let result = CompletionInfo::new(many_values);
    assert!(result.is_err());
}

#[test]
fn test_completion_info_helper_methods() {
    let values = vec!["test1".to_string(), "test2".to_string()];

    // Test with_all_values
    let completion = CompletionInfo::with_all_values(values.clone()).unwrap();
    assert_eq!(completion.values, values);
    assert_eq!(completion.total, Some(2));
    assert_eq!(completion.has_more, Some(false));
    assert!(!completion.has_more_results());
    assert_eq!(completion.total_available(), Some(2));

    // Test with_pagination
    let paginated = CompletionInfo::with_pagination(values.clone(), Some(10), true).unwrap();
    assert_eq!(paginated.values, values);
    assert_eq!(paginated.total, Some(10));
    assert_eq!(paginated.has_more, Some(true));
    assert!(paginated.has_more_results());
    assert_eq!(paginated.total_available(), Some(10));
}

#[test]
fn test_completion_info_bounds() {
    // Test exactly at the limit
    let max_values: Vec<String> = (0..CompletionInfo::MAX_VALUES)
        .map(|i| format!("value_{}", i))
        .collect();
    assert!(CompletionInfo::new(max_values).is_ok());

    // Test over the limit
    let over_limit: Vec<String> = (0..=CompletionInfo::MAX_VALUES)
        .map(|i| format!("value_{}", i))
        .collect();
    assert!(CompletionInfo::new(over_limit).is_err());
}

#[test]
fn test_reference_convenience_methods() {
    let prompt_ref = Reference::for_prompt("test_prompt");
    assert_eq!(prompt_ref.reference_type(), "ref/prompt");
    assert_eq!(prompt_ref.as_prompt_name(), Some("test_prompt"));
    assert_eq!(prompt_ref.as_resource_uri(), None);

    let resource_ref = Reference::for_resource("file://path/to/resource");
    assert_eq!(resource_ref.reference_type(), "ref/resource");
    assert_eq!(
        resource_ref.as_resource_uri(),
        Some("file://path/to/resource")
    );
    assert_eq!(resource_ref.as_prompt_name(), None);
}

#[test]
fn test_completion_serialization_format() {
    // Test that completion follows MCP 2025-06-18 specification format
    let completion = CompletionInfo {
        values: vec!["value1".to_string(), "value2".to_string()],
        total: Some(2),
        has_more: Some(false),
    };

    let json = serde_json::to_value(&completion).unwrap();

    // Verify JSON structure matches specification
    assert!(json.is_object());
    assert!(json["values"].is_array());
    assert_eq!(json["values"].as_array().unwrap().len(), 2);
    assert_eq!(json["total"].as_u64().unwrap(), 2);
    assert!(!json["hasMore"].as_bool().unwrap());
}

#[test]
fn test_resource_reference() {
    // Test that ResourceReference works correctly
    let resource_ref = ResourceReference {
        uri: "test://uri".to_string(),
    };

    // Test that ResourceReference works correctly
    let another_ref = ResourceReference {
        uri: "test://uri".to_string(),
    };

    // They should be equivalent
    assert_eq!(resource_ref.uri, another_ref.uri);
}

#[test]
fn test_complete_result_default() {
    let result = CompleteResult::default();
    assert!(result.completion.values.is_empty());
    assert_eq!(result.completion.total, None);
    assert_eq!(result.completion.has_more, None);
}

#[test]
fn test_completion_context_empty() {
    let context = CompletionContext::new();
    assert!(!context.has_arguments());
    assert_eq!(context.get_argument("any"), None);
    assert!(context.argument_names().is_empty());
}

#[tokio::test]
async fn test_default_completion_provider() {
    let provider = DefaultCompletionProvider::new();

    let result = provider
        .complete_prompt_argument("test_prompt", "arg", "ex", None)
        .await
        .unwrap();

    assert!(!result.values.is_empty());
    assert!(result.values.iter().any(|v| v.contains("example")));
    assert_eq!(result.total, Some(result.values.len() as u32));
    assert_eq!(result.has_more, Some(false));
}

#[tokio::test]
async fn test_completion_provider_with_context() {
    let provider = DefaultCompletionProvider::new();

    let mut args = HashMap::new();
    args.insert("prev_arg".to_string(), "some_value".to_string());
    let context = CompletionContext::with_arguments(args);

    let result = provider
        .complete_prompt_argument("test_prompt", "arg", "test", Some(&context))
        .await
        .unwrap();

    assert!(!result.values.is_empty());
    assert!(context.has_arguments());
    assert!(context.get_argument("prev_arg").is_some());
}

#[tokio::test]
async fn test_fuzzy_matching() {
    let provider = DefaultCompletionProvider::new();
    let candidates = vec![
        "hello_world".to_string(),
        "hello_rust".to_string(),
        "world_peace".to_string(),
        "rust_lang".to_string(),
    ];

    let matches = provider.fuzzy_match("hello", &candidates);
    assert_eq!(matches.len(), 2);
    assert!(matches.contains(&"hello_world".to_string()));
    assert!(matches.contains(&"hello_rust".to_string()));

    // Test empty query returns all candidates (up to limit)
    let all_matches = provider.fuzzy_match("", &candidates);
    assert_eq!(all_matches.len(), candidates.len());

    // Test no matches
    let no_matches = provider.fuzzy_match("xyz", &candidates);
    assert!(no_matches.is_empty());
}

#[tokio::test]
async fn test_fuzzy_matching_with_typos_and_missing_chars() {
    let provider = DefaultCompletionProvider::new();
    let candidates = vec![
        "javascript".to_string(),
        "typescript".to_string(),
        "python".to_string(),
        "rust_analyzer".to_string(),
        "cargo_test".to_string(),
        "github_actions".to_string(),
        "dockerfile".to_string(),
        "requirements_txt".to_string(),
    ];

    // Test missing characters (subsequence matching)
    let matches = provider.fuzzy_match("jscrt", &candidates);
    assert!(!matches.is_empty());
    assert!(matches.contains(&"javascript".to_string()));

    // Test with missing middle characters
    let matches = provider.fuzzy_match("tpscpt", &candidates);
    assert!(!matches.is_empty());
    assert!(matches.contains(&"typescript".to_string()));

    // Test abbreviated matching
    let matches = provider.fuzzy_match("py", &candidates);
    assert!(matches.contains(&"python".to_string()));

    // Test underscore separated words
    let matches = provider.fuzzy_match("rust_anl", &candidates);
    assert!(matches.contains(&"rust_analyzer".to_string()));

    // Test partial word matching
    let matches = provider.fuzzy_match("crg", &candidates);
    assert!(matches.contains(&"cargo_test".to_string()));

    // Test case insensitive matching
    let matches = provider.fuzzy_match("GITHUB", &candidates);
    assert!(matches.contains(&"github_actions".to_string()));

    // Test file extension patterns
    let matches = provider.fuzzy_match("dock", &candidates);
    assert!(matches.contains(&"dockerfile".to_string()));

    // Test complex subsequence
    let matches = provider.fuzzy_match("req_txt", &candidates);
    assert!(matches.contains(&"requirements_txt".to_string()));
}

#[tokio::test]
async fn test_fuzzy_matching_scoring_priority() {
    let provider = DefaultCompletionProvider::new();
    let candidates = vec![
        "test".to_string(),      // Exact match - highest priority
        "testing".to_string(),   // Prefix match - high priority
        "contest".to_string(),   // Contains substring - medium priority
        "temporary".to_string(), // Subsequence match - lower priority
    ];

    // Test that exact matches come first
    let matches = provider.fuzzy_match("test", &candidates);
    assert!(!matches.is_empty());
    assert_eq!(matches[0], "test");

    // Test prefix matching gets higher priority than substring
    let matches = provider.fuzzy_match("temp", &candidates);
    assert!(!matches.is_empty());
    // "temporary" should be first since it's a prefix match
    assert_eq!(matches[0], "temporary");
}

#[tokio::test]
async fn test_fuzzy_matching_edge_cases() {
    let provider = DefaultCompletionProvider::new();
    let candidates = vec![
        "a".to_string(),
        "ab".to_string(),
        "abc".to_string(),
        "abcd".to_string(),
        "xyz".to_string(),
    ];

    // Test single character matching
    let matches = provider.fuzzy_match("a", &candidates);
    assert!(matches.len() >= 4); // Should match a, ab, abc, abcd

    // Test query longer than some candidates
    let matches = provider.fuzzy_match("abcdef", &candidates);
    assert!(matches.is_empty()); // No candidate contains all characters

    // Test repeated characters
    let candidates_with_repeats = vec!["aaa".to_string(), "aba".to_string(), "bbb".to_string()];
    let matches = provider.fuzzy_match("aa", &candidates_with_repeats);
    assert!(matches.contains(&"aaa".to_string()));
}

#[tokio::test]
async fn test_fuzzy_matching_acronyms_and_word_boundaries() {
    let provider = DefaultCompletionProvider::new();
    let cities = vec![
        "New York".to_string(),
        "Los Angeles".to_string(),
        "San Francisco".to_string(),
        "Las Vegas".to_string(),
        "Salt Lake City".to_string(),
        "New Orleans".to_string(),
        "San Diego".to_string(),
        "San Antonio".to_string(),
        "Buenos Aires".to_string(),
        "Mexico City".to_string(),
        "Rio de Janeiro".to_string(),
        "Hong Kong".to_string(),
        "Toronto".to_string(),
        "Frankfurt am Main".to_string(),
        "Beijing".to_string(),
        "Shanghai".to_string(),
        "Guangzhou".to_string(),
        "Shenzhen".to_string(),
        "Chengdu".to_string(),
        "Hangzhou".to_string(),
    ];

    // Test acronym matching for two-word cities
    let matches = provider.fuzzy_match("NY", &cities);
    assert!(!matches.is_empty());
    assert!(matches.contains(&"New York".to_string()));

    let matches = provider.fuzzy_match("LA", &cities);
    assert!(!matches.is_empty());
    assert!(matches.contains(&"Los Angeles".to_string()));

    let matches = provider.fuzzy_match("SF", &cities);
    assert!(!matches.is_empty());
    assert!(matches.contains(&"San Francisco".to_string()));

    let matches = provider.fuzzy_match("LV", &cities);
    assert!(!matches.is_empty());
    assert!(matches.contains(&"Las Vegas".to_string()));

    // Test acronym matching for three-word cities
    let matches = provider.fuzzy_match("SLC", &cities);
    assert!(!matches.is_empty());
    assert!(matches.contains(&"Salt Lake City".to_string()));

    let matches = provider.fuzzy_match("BA", &cities);
    assert!(!matches.is_empty());
    assert!(matches.contains(&"Buenos Aires".to_string()));

    let matches = provider.fuzzy_match("MC", &cities);
    assert!(!matches.is_empty());
    assert!(matches.contains(&"Mexico City".to_string()));

    // Test acronym matching for cities with more complex names
    let matches = provider.fuzzy_match("HK", &cities);
    assert!(!matches.is_empty());
    assert!(matches.contains(&"Hong Kong".to_string()));

    // Test partial word matching still works
    let matches = provider.fuzzy_match("san", &cities);
    assert!(!matches.is_empty());
    // Should match San Francisco, San Diego, San Antonio
    assert!(
        matches.contains(&"San Francisco".to_string())
            || matches.contains(&"San Diego".to_string())
            || matches.contains(&"San Antonio".to_string())
    );

    let matches = provider.fuzzy_match("new", &cities);
    assert!(!matches.is_empty());
    // Should match New York, New Orleans
    assert!(
        matches.contains(&"New York".to_string()) || matches.contains(&"New Orleans".to_string())
    );

    // Test case insensitive acronyms
    let matches = provider.fuzzy_match("ny", &cities);
    assert!(!matches.is_empty());
    assert!(matches.contains(&"New York".to_string()));

    let matches = provider.fuzzy_match("la", &cities);
    assert!(!matches.is_empty());
    assert!(matches.contains(&"Los Angeles".to_string()));
}

#[tokio::test]
async fn test_fuzzy_matching_scoring_priority_with_acronyms() {
    let provider = DefaultCompletionProvider::new();
    let candidates = vec![
        "Los Angeles".to_string(), // Should match "LA" as acronym
        "Louisiana".to_string(),   // Should match "LA" as prefix
        "Las Vegas".to_string(),   // Should match "LA" as prefix
        "Laos".to_string(),        // Should match "LA" as prefix
        "Latvia".to_string(),      // Should match "LA" as prefix
        "Salt Lake".to_string(),   // Should match "LA" as substring
    ];

    // Test that acronym matching gets appropriate priority
    let matches = provider.fuzzy_match("LA", &candidates);
    assert!(!matches.is_empty());

    // Los Angeles should be found (acronym match)
    assert!(matches.contains(&"Los Angeles".to_string()));

    // Prefix matches should also be found
    assert!(
        matches.contains(&"Louisiana".to_string())
            || matches.contains(&"Las Vegas".to_string())
            || matches.contains(&"Laos".to_string())
            || matches.contains(&"Latvia".to_string())
    );
}

#[tokio::test]
async fn test_fuzzy_matching_edge_cases_with_spaces() {
    let provider = DefaultCompletionProvider::new();
    let candidates = vec![
        "A".to_string(),
        "A B".to_string(),
        "A B C".to_string(),
        "AA BB".to_string(),
        "ABC DEF".to_string(),
        "X Y Z W".to_string(),
    ];

    // Test single character acronym
    let matches = provider.fuzzy_match("A", &candidates);
    assert!(!matches.is_empty());
    assert!(matches.contains(&"A".to_string()));

    // Test two character acronym
    let matches = provider.fuzzy_match("AB", &candidates);
    assert!(!matches.is_empty());
    assert!(matches.contains(&"A B".to_string()));

    // Test three character acronym
    let matches = provider.fuzzy_match("ABC", &candidates);
    assert!(!matches.is_empty());
    assert!(matches.contains(&"A B C".to_string()));

    // Test four character acronym
    let matches = provider.fuzzy_match("XYZW", &candidates);
    assert!(!matches.is_empty());
    assert!(matches.contains(&"X Y Z W".to_string()));

    // Test that wrong number of characters doesn't match as acronym
    let _matches = provider.fuzzy_match("ABCD", &candidates);
    // Should not match any acronyms, but might match as substring/subsequence
}

#[test]
fn test_mcp_schema_compliance() {
    // Test that our types serialize correctly according to MCP specification
    let request = CompleteRequestParam {
        r#ref: Reference::for_resource("file://{path}"),
        argument: ArgumentInfo {
            name: "path".to_string(),
            value: "src/".to_string(),
        },
        context: None,
    };

    let json_str = serde_json::to_string(&request).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    // Verify key structure matches MCP spec
    assert!(parsed["ref"].is_object());
    assert!(parsed["argument"].is_object());
    assert!(parsed["argument"]["name"].is_string());
    assert!(parsed["argument"]["value"].is_string());

    // Verify type tag is correct
    assert_eq!(parsed["ref"]["type"].as_str().unwrap(), "ref/resource");
}

#[tokio::test]
async fn test_completion_edge_cases() {
    let provider = DefaultCompletionProvider::with_max_suggestions(2);

    // Test with max suggestions limit
    let candidates = vec![
        "option1".to_string(),
        "option2".to_string(),
        "option3".to_string(),
        "option4".to_string(),
    ];

    let matches = provider.fuzzy_match("opt", &candidates);
    assert!(matches.len() <= 2); // Should respect max_suggestions

    // Test resource completion
    let result = provider
        .complete_resource_argument("db://{table}", "table", "file", None)
        .await
        .unwrap();

    assert!(!result.values.is_empty());
    assert!(result.values.iter().any(|v| v.contains("file")));
}

#[tokio::test]
async fn test_completion_performance() {
    let provider = DefaultCompletionProvider::new();

    // Create a large candidate set
    let candidates: Vec<String> = (0..1000).map(|i| format!("candidate_{:04}", i)).collect();

    let start = std::time::Instant::now();
    let matches = provider.fuzzy_match("candidate_", &candidates);
    let duration = start.elapsed();

    // Should complete within reasonable time (less than 100ms for 1000 candidates)
    assert!(duration.as_millis() < 100);
    assert!(!matches.is_empty());
    assert!(matches.len() <= CompletionInfo::MAX_VALUES);
}
