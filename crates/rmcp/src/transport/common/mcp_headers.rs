//! SEP-2243 HTTP header standardization.
//!
//! Builds and validates the `Mcp-Method`, `Mcp-Name`, and `Mcp-Param-*` headers
//! so middle boxes can route Streamable HTTP traffic without parsing the body.
//! All emission/validation is gated by the negotiated protocol version
//! (`>= ProtocolVersion::STANDARD_HEADERS`) at the call sites.

// Which helpers are reachable depends on the client/server feature combination,
// mirroring `server_side_http`.
#![allow(dead_code)]

use serde_json::Value;

use super::http_header::{
    BASE64_HEADER_PREFIX, BASE64_HEADER_SUFFIX, HEADER_MCP_METHOD, HEADER_MCP_NAME,
    HEADER_MCP_PARAM_PREFIX,
};
use crate::model::JsonObject;

/// Methods whose `Mcp-Name` is sourced from `params.name`.
const NAME_FROM_NAME: &[&str] = &["tools/call", "prompts/get"];
/// Methods whose `Mcp-Name` is sourced from `params.uri`.
const NAME_FROM_URI: &[&str] = &[
    "resources/read",
    "resources/subscribe",
    "resources/unsubscribe",
];

/// Returns the `Mcp-Name` value for a request, if the method carries one.
fn extract_name(method: &str, params: Option<&Value>) -> Option<String> {
    let params = params?;
    let key = if NAME_FROM_NAME.contains(&method) {
        "name"
    } else if NAME_FROM_URI.contains(&method) {
        "uri"
    } else {
        return None;
    };
    params.get(key)?.as_str().map(str::to_owned)
}

/// Converts a JSON primitive to its SEP-2243 string form. Non-primitives yield `None`.
fn primitive_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// True if `value` must be Base64-wrapped to survive as an HTTP header value:
/// leading/trailing space or tab, control/non-ASCII characters, or a value that
/// already looks like the `=?base64?...?=` sentinel.
#[cfg(feature = "client-side-sse")]
fn requires_base64(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    let bytes = value.as_bytes();
    if matches!(bytes.first(), Some(b' ' | b'\t')) || matches!(bytes.last(), Some(b' ' | b'\t')) {
        return true;
    }
    if value
        .chars()
        .any(|c| (c as u32) < 0x20 || (c as u32) > 0x7E)
    {
        return true;
    }
    value.starts_with(BASE64_HEADER_PREFIX) && value.ends_with(BASE64_HEADER_SUFFIX)
}

/// RFC 9110 §5.6.2 token character.
#[cfg(feature = "client-side-sse")]
fn is_tchar(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '!' | '#'
                | '$'
                | '%'
                | '&'
                | '\''
                | '*'
                | '+'
                | '-'
                | '.'
                | '^'
                | '_'
                | '`'
                | '|'
                | '~'
        )
}

/// Top-level properties carrying an `x-mcp-header` annotation, as `(property, header)` pairs.
fn param_header_annotations(input_schema: &JsonObject) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Some(Value::Object(props)) = input_schema.get("properties") {
        for (prop, schema) in props {
            if let Some(Value::String(header)) = schema.get("x-mcp-header") {
                if !header.is_empty() {
                    out.push((prop.clone(), header.clone()));
                }
            }
        }
    }
    out
}

/// Validates the `x-mcp-header` annotations in a tool input schema.
///
/// Annotations must be non-empty RFC 9110 tokens, case-insensitively unique,
/// applied only to top-level primitive (`string`/`integer`/`boolean`) properties.
/// Returns the offending reason on the first violation.
#[cfg(feature = "client-side-sse")]
pub(crate) fn validate_param_header_annotations(input_schema: &JsonObject) -> Result<(), String> {
    let Some(Value::Object(props)) = input_schema.get("properties") else {
        return Ok(());
    };
    let mut seen = std::collections::HashSet::new();
    for (prop, schema) in props {
        reject_nested_annotations(schema, prop)?;
        let Some(raw) = schema.get("x-mcp-header") else {
            continue;
        };
        let Value::String(header) = raw else {
            return Err(format!("property `{prop}`: x-mcp-header must be a string"));
        };
        if header.is_empty() {
            return Err(format!("property `{prop}`: x-mcp-header must not be empty"));
        }
        if !header.chars().all(is_tchar) {
            return Err(format!(
                "property `{prop}`: x-mcp-header `{header}` is not a valid HTTP token"
            ));
        }
        if !seen.insert(header.to_ascii_lowercase()) {
            return Err(format!(
                "property `{prop}`: duplicate x-mcp-header `{header}` (case-insensitive)"
            ));
        }
        match schema.get("type").and_then(Value::as_str) {
            Some("string" | "integer" | "boolean") => {}
            other => {
                return Err(format!(
                    "property `{prop}`: x-mcp-header requires a primitive type \
                     (string/integer/boolean), got {other:?}"
                ));
            }
        }
    }
    Ok(())
}

/// Rejects `x-mcp-header` on nested properties (only top-level promotion is supported).
#[cfg(feature = "client-side-sse")]
fn reject_nested_annotations(schema: &Value, path: &str) -> Result<(), String> {
    if let Some(Value::Object(nested)) = schema.get("properties") {
        for (key, value) in nested {
            if value.get("x-mcp-header").is_some() {
                return Err(format!(
                    "property `{path}.{key}`: x-mcp-header is not supported on nested properties"
                ));
            }
            reject_nested_annotations(value, &format!("{path}.{key}"))?;
        }
    }
    Ok(())
}

/// Wraps a value as `=?base64?<b64>?=` when it cannot travel as a bare header value.
#[cfg(feature = "client-side-sse")]
fn encode_header_value(value: &str) -> String {
    use base64::{Engine, prelude::BASE64_STANDARD};
    if requires_base64(value) {
        format!(
            "{BASE64_HEADER_PREFIX}{}{BASE64_HEADER_SUFFIX}",
            BASE64_STANDARD.encode(value)
        )
    } else {
        value.to_owned()
    }
}

/// Reverses [`encode_header_value`]. Returns `None` if the sentinel wraps invalid Base64/UTF-8.
#[cfg(feature = "server-side-http")]
fn decode_header_value(value: &str) -> Option<String> {
    use base64::{Engine, prelude::BASE64_STANDARD};
    match value
        .strip_prefix(BASE64_HEADER_PREFIX)
        .and_then(|inner| inner.strip_suffix(BASE64_HEADER_SUFFIX))
    {
        Some(inner) => {
            let bytes = BASE64_STANDARD.decode(inner).ok()?;
            String::from_utf8(bytes).ok()
        }
        None => Some(value.to_owned()),
    }
}

/// Builds the SEP-2243 headers for an outgoing request from its JSON form.
///
/// `tool_schema` is the cached input schema of the called tool, used to promote
/// annotated `tools/call` arguments to `Mcp-Param-*` headers.
#[cfg(feature = "client-side-sse")]
pub(crate) fn standard_request_headers(
    request: &Value,
    tool_schema: Option<&JsonObject>,
) -> Vec<(http::HeaderName, http::HeaderValue)> {
    use http::{HeaderName, HeaderValue};

    let mut out = Vec::new();
    let Some(method) = request.get("method").and_then(Value::as_str) else {
        return out;
    };
    let params = request.get("params");

    let mut push = |name: &str, value: &str| {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            out.push((name, value));
        }
    };

    push(HEADER_MCP_METHOD, method);
    if let Some(name) = extract_name(method, params) {
        push(HEADER_MCP_NAME, &encode_header_value(&name));
    }

    if method == "tools/call" {
        if let (Some(schema), Some(arguments)) =
            (tool_schema, params.and_then(|p| p.get("arguments")))
        {
            for (prop, header) in param_header_annotations(schema) {
                let Some(arg) = arguments.get(&prop) else {
                    continue;
                };
                let Some(encoded) = primitive_to_string(arg).map(|s| encode_header_value(&s))
                else {
                    continue;
                };
                push(&format!("{HEADER_MCP_PARAM_PREFIX}{header}"), &encoded);
            }
        }
    }
    out
}

/// Validates incoming SEP-2243 headers against the request body.
///
/// Returns `Err(reason)` when a required header is missing or its value does not
/// match the body; the caller maps this to a JSON-RPC `-32020` error (HTTP 400).
#[cfg(feature = "server-side-http")]
pub(crate) fn validate_request_headers(
    headers: &http::HeaderMap,
    request: &Value,
    tool_schema: Option<&JsonObject>,
) -> Result<(), String> {
    let Some(method) = request.get("method").and_then(Value::as_str) else {
        return Ok(());
    };
    let params = request.get("params");

    let header_method = header_str(headers, HEADER_MCP_METHOD);
    match header_method {
        None => return Err("missing required Mcp-Method header".to_owned()),
        Some(value) if value != method => {
            return Err(format!(
                "Mcp-Method header `{value}` does not match body method `{method}`"
            ));
        }
        Some(_) => {}
    }

    if let Some(expected) = extract_name(method, params) {
        match header_str(headers, HEADER_MCP_NAME) {
            None => return Err(format!("missing required Mcp-Name header for `{method}`")),
            Some(raw) => {
                let decoded = decode_header_value(raw)
                    .ok_or_else(|| "Mcp-Name header is not valid Base64".to_owned())?;
                if decoded != expected {
                    return Err(format!(
                        "Mcp-Name header `{decoded}` does not match body value `{expected}`"
                    ));
                }
            }
        }
    }

    if method == "tools/call" {
        if let Some(schema) = tool_schema {
            let arguments = params.and_then(|p| p.get("arguments"));
            for (prop, header) in param_header_annotations(schema) {
                let full = format!("{HEADER_MCP_PARAM_PREFIX}{header}");
                let header_value = header_str(headers, &full);
                let arg = arguments.and_then(|a| a.get(&prop));
                let body_value = arg.filter(|v| !v.is_null()).and_then(primitive_to_string);

                match (header_value, body_value) {
                    (None, None) => {}
                    (Some(_), None) => {
                        return Err(format!(
                            "unexpected {full} header for absent or null `{prop}`"
                        ));
                    }
                    (None, Some(_)) => {
                        return Err(format!("missing {full} header for `{prop}`"));
                    }
                    (Some(raw), Some(expected)) => {
                        let decoded = decode_header_value(raw)
                            .ok_or_else(|| format!("{full} header is not valid Base64"))?;
                        if decoded != expected {
                            return Err(format!(
                                "{full} header `{decoded}` does not match body value `{expected}`"
                            ));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Case-insensitive header lookup returning the value as `&str`, if present and valid UTF-8.
#[cfg(feature = "server-side-http")]
fn header_str<'a>(headers: &'a http::HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

#[cfg(all(test, feature = "client-side-sse", feature = "server-side-http"))]
mod tests {
    use std::collections::HashMap;

    use http::{HeaderMap, HeaderName, HeaderValue};
    use serde_json::json;

    use super::*;

    fn schema_with(properties: serde_json::Value) -> JsonObject {
        json!({ "type": "object", "properties": properties })
            .as_object()
            .unwrap()
            .clone()
    }

    fn header_map(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    fn assert_wrapped(value: &str) {
        let encoded = encode_header_value(value);
        assert!(
            encoded.starts_with(BASE64_HEADER_PREFIX),
            "expected {value:?} to be Base64-wrapped, got {encoded:?}"
        );
    }

    mod encode_header_value {
        use super::*;

        #[test]
        fn passes_plain_ascii_through() {
            assert_eq!(encode_header_value("us-west1"), "us-west1");
        }

        #[test]
        fn passes_internal_spaces_through() {
            assert_eq!(encode_header_value("a b c"), "a b c");
        }

        #[test]
        fn wraps_non_ascii() {
            assert_wrapped("café");
        }

        #[test]
        fn wraps_leading_whitespace() {
            assert_wrapped(" padded");
        }

        #[test]
        fn wraps_trailing_whitespace() {
            assert_wrapped("trailing ");
        }

        #[test]
        fn wraps_control_characters() {
            assert_wrapped("line1\nline2");
        }

        #[test]
        fn wraps_crlf_injection_attempt() {
            assert_wrapped("a\r\nEvil: 1");
        }

        #[test]
        fn wraps_sentinel_collision() {
            assert_wrapped(&format!("{BASE64_HEADER_PREFIX}x{BASE64_HEADER_SUFFIX}"));
        }
    }

    mod decode_header_value {
        use super::*;

        #[test]
        fn round_trips_with_encode() {
            for value in ["us-west1", "café", " padded ", "line1\nline2", "true", "42"] {
                let encoded = encode_header_value(value);
                assert_eq!(
                    decode_header_value(&encoded).as_deref(),
                    Some(value),
                    "round-trip failed for {value:?}"
                );
            }
        }

        #[test]
        fn rejects_invalid_base64() {
            let bad = format!("{BASE64_HEADER_PREFIX}!!!not-base64!!!{BASE64_HEADER_SUFFIX}");
            assert_eq!(decode_header_value(&bad), None);
        }
    }

    mod extract_name {
        use super::*;

        #[test]
        fn from_name_for_tools_call() {
            let params = json!({ "name": "my_tool" });
            assert_eq!(
                extract_name("tools/call", Some(&params)).as_deref(),
                Some("my_tool")
            );
        }

        #[test]
        fn from_name_for_prompts_get() {
            let params = json!({ "name": "my_prompt" });
            assert_eq!(
                extract_name("prompts/get", Some(&params)).as_deref(),
                Some("my_prompt")
            );
        }

        #[test]
        fn from_uri_for_resources_read() {
            let params = json!({ "uri": "file:///x" });
            assert_eq!(
                extract_name("resources/read", Some(&params)).as_deref(),
                Some("file:///x")
            );
        }

        #[test]
        fn none_for_unrelated_method() {
            let params = json!({ "name": "my_tool" });
            assert_eq!(extract_name("ping", Some(&params)), None);
        }

        #[test]
        fn none_when_params_absent() {
            assert_eq!(extract_name("tools/call", None), None);
        }
    }

    mod validate_param_header_annotations {
        use super::*;

        #[test]
        fn accepts_primitive_types() {
            let schema = schema_with(json!({
                "region": { "type": "string", "x-mcp-header": "Region" },
                "count": { "type": "integer", "x-mcp-header": "Count" },
                "flag": { "type": "boolean", "x-mcp-header": "Flag" },
            }));
            assert!(validate_param_header_annotations(&schema).is_ok());
        }

        #[test]
        fn rejects_number_type() {
            let schema = schema_with(json!({ "n": { "type": "number", "x-mcp-header": "N" } }));
            assert!(validate_param_header_annotations(&schema).is_err());
        }

        #[test]
        fn rejects_complex_type() {
            let schema = schema_with(json!({ "a": { "type": "array", "x-mcp-header": "A" } }));
            assert!(validate_param_header_annotations(&schema).is_err());
        }

        #[test]
        fn rejects_empty_header_name() {
            let schema = schema_with(json!({ "r": { "type": "string", "x-mcp-header": "" } }));
            assert!(validate_param_header_annotations(&schema).is_err());
        }

        #[test]
        fn rejects_non_token_header_name() {
            let schema =
                schema_with(json!({ "r": { "type": "string", "x-mcp-header": "bad:name" } }));
            assert!(validate_param_header_annotations(&schema).is_err());
        }

        #[test]
        fn rejects_case_insensitive_duplicate() {
            let schema = schema_with(json!({
                "a": { "type": "string", "x-mcp-header": "Region" },
                "b": { "type": "string", "x-mcp-header": "region" },
            }));
            assert!(validate_param_header_annotations(&schema).is_err());
        }

        #[test]
        fn rejects_nested_annotation() {
            let schema = schema_with(json!({
                "outer": {
                    "type": "object",
                    "properties": { "inner": { "type": "string", "x-mcp-header": "Inner" } }
                }
            }));
            assert!(validate_param_header_annotations(&schema).is_err());
        }
    }

    mod standard_request_headers {
        use super::*;

        fn tools_call_headers() -> HashMap<String, String> {
            let schema = schema_with(json!({
                "region": { "type": "string", "x-mcp-header": "Region" },
            }));
            let request = json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": "deploy", "arguments": { "region": "us-west1" } }
            });
            super::super::standard_request_headers(&request, Some(&schema))
                .into_iter()
                .map(|(name, value)| (name.as_str().to_owned(), value.to_str().unwrap().to_owned()))
                .collect()
        }

        #[test]
        fn sets_method_header() {
            assert_eq!(
                tools_call_headers().get("mcp-method").map(String::as_str),
                Some("tools/call")
            );
        }

        #[test]
        fn sets_name_header() {
            assert_eq!(
                tools_call_headers().get("mcp-name").map(String::as_str),
                Some("deploy")
            );
        }

        #[test]
        fn sets_annotated_param_header() {
            assert_eq!(
                tools_call_headers()
                    .get("mcp-param-region")
                    .map(String::as_str),
                Some("us-west1")
            );
        }
    }

    mod validate_request_headers {
        use super::*;

        fn tools_call_request() -> Value {
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": "deploy" }
            })
        }

        #[test]
        fn accepts_matching_method_and_name() {
            let headers = header_map(&[("Mcp-Method", "tools/call"), ("Mcp-Name", "deploy")]);
            assert!(validate_request_headers(&headers, &tools_call_request(), None).is_ok());
        }

        #[test]
        fn rejects_method_mismatch() {
            let headers = header_map(&[("Mcp-Method", "tools/list"), ("Mcp-Name", "deploy")]);
            assert!(validate_request_headers(&headers, &tools_call_request(), None).is_err());
        }

        #[test]
        fn rejects_missing_method() {
            let headers = header_map(&[("Mcp-Name", "deploy")]);
            assert!(validate_request_headers(&headers, &tools_call_request(), None).is_err());
        }

        #[test]
        fn rejects_name_mismatch() {
            let headers = header_map(&[("Mcp-Method", "tools/call"), ("Mcp-Name", "other")]);
            assert!(validate_request_headers(&headers, &tools_call_request(), None).is_err());
        }

        #[test]
        fn rejects_missing_name() {
            let headers = header_map(&[("Mcp-Method", "tools/call")]);
            assert!(validate_request_headers(&headers, &tools_call_request(), None).is_err());
        }

        #[test]
        fn accepts_matching_param() {
            let schema = schema_with(json!({
                "region": { "type": "string", "x-mcp-header": "Region" },
            }));
            let request = json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": "deploy", "arguments": { "region": "us-west1" } }
            });
            let headers = header_map(&[
                ("Mcp-Method", "tools/call"),
                ("Mcp-Name", "deploy"),
                ("Mcp-Param-Region", "us-west1"),
            ]);
            assert!(validate_request_headers(&headers, &request, Some(&schema)).is_ok());
        }

        #[test]
        fn rejects_param_mismatch() {
            let schema = schema_with(json!({
                "region": { "type": "string", "x-mcp-header": "Region" },
            }));
            let request = json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": "deploy", "arguments": { "region": "us-west1" } }
            });
            let headers = header_map(&[
                ("Mcp-Method", "tools/call"),
                ("Mcp-Name", "deploy"),
                ("Mcp-Param-Region", "eu-central1"),
            ]);
            assert!(validate_request_headers(&headers, &request, Some(&schema)).is_err());
        }
    }
}
