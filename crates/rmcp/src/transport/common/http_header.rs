pub const HEADER_SESSION_ID: &str = "Mcp-Session-Id";
pub const HEADER_LAST_EVENT_ID: &str = "Last-Event-Id";
pub const HEADER_MCP_PROTOCOL_VERSION: &str = "MCP-Protocol-Version";
pub const EVENT_STREAM_MIME_TYPE: &str = "text/event-stream";
pub const JSON_MIME_TYPE: &str = "application/json";

// SEP-2243 standard headers, gated on protocol version >= 2026-07-28.
pub const HEADER_MCP_METHOD: &str = "Mcp-Method";
pub const HEADER_MCP_NAME: &str = "Mcp-Name";
pub const HEADER_MCP_PARAM_PREFIX: &str = "Mcp-Param-";
pub const HEADER_MCP_PARAM_PREFIX_LOWER: &str = "mcp-param-";

#[cfg(any(feature = "client-side-sse", feature = "server-side-http"))]
pub const HEADER_NAME_SESSION_ID: http::HeaderName =
    http::HeaderName::from_static("mcp-session-id");
#[cfg(any(feature = "client-side-sse", feature = "server-side-http"))]
pub const HEADER_NAME_LAST_EVENT_ID: http::HeaderName =
    http::HeaderName::from_static("last-event-id");
#[cfg(any(feature = "client-side-sse", feature = "server-side-http"))]
pub const HEADER_NAME_MCP_PROTOCOL_VERSION: http::HeaderName =
    http::HeaderName::from_static("mcp-protocol-version");
#[cfg(any(feature = "client-side-sse", feature = "server-side-http"))]
pub const HEADER_NAME_MCP_METHOD: http::HeaderName = http::HeaderName::from_static("mcp-method");
#[cfg(any(feature = "client-side-sse", feature = "server-side-http"))]
pub const HEADER_NAME_MCP_NAME: http::HeaderName = http::HeaderName::from_static("mcp-name");

/// Sentinel wrapping a Base64-encoded SEP-2243 header value (`=?base64?<b64>?=`).
pub const BASE64_HEADER_PREFIX: &str = "=?base64?";
pub const BASE64_HEADER_SUFFIX: &str = "?=";

/// Reserved headers that must not be overridden by user-supplied custom headers.
/// `MCP-Protocol-Version` is in this list but is allowed through because the worker
/// injects it after initialization.
#[allow(dead_code)]
pub(crate) const RESERVED_HEADERS: &[&str] = &[
    "accept",
    HEADER_SESSION_ID,
    HEADER_MCP_PROTOCOL_VERSION, // allowed through by validate_custom_header; worker injects it post-init
    HEADER_LAST_EVENT_ID,
];

/// Checks whether a custom header name is allowed.
/// Returns `Ok(())` if allowed, `Err(name)` if rejected as reserved.
/// `MCP-Protocol-Version` is reserved but allowed through (the worker injects it post-init).
#[cfg(feature = "client-side-sse")]
pub(crate) fn validate_custom_header(name: &http::HeaderName) -> Result<(), String> {
    if is_reserved_header_name(name) {
        if name == HEADER_NAME_MCP_PROTOCOL_VERSION {
            return Ok(());
        }
        return Err(name.to_string());
    }
    Ok(())
}

#[cfg(feature = "client-side-sse")]
fn is_reserved_header_name(name: &http::HeaderName) -> bool {
    name == http::header::ACCEPT
        || name == HEADER_NAME_SESSION_ID
        || name == HEADER_NAME_MCP_PROTOCOL_VERSION
        || name == HEADER_NAME_LAST_EVENT_ID
}

/// Extracts the `scope=` parameter from a `WWW-Authenticate` header value.
/// Handles both quoted (`scope="files:read files:write"`) and unquoted (`scope=read:data`) forms.
#[cfg(feature = "client-side-sse")]
pub(crate) fn extract_scope_from_header(header: &str) -> Option<String> {
    let header_lowercase = header.to_ascii_lowercase();
    let scope_key = "scope=";

    if let Some(pos) = header_lowercase.find(scope_key) {
        let start = pos + scope_key.len();
        let value_slice = &header[start..];

        if let Some(stripped) = value_slice.strip_prefix('"') {
            if let Some(end_quote) = stripped.find('"') {
                return Some(stripped[..end_quote].to_string());
            }
        } else {
            let end = value_slice
                .find(|c: char| c == ',' || c == ';' || c.is_whitespace())
                .unwrap_or(value_slice.len());
            if end > 0 {
                return Some(value_slice[..end].to_string());
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    #[cfg(any(feature = "client-side-sse", feature = "server-side-http"))]
    use http::{HeaderMap, HeaderName, HeaderValue};

    #[cfg(feature = "client-side-sse")]
    use super::*;

    #[cfg(feature = "client-side-sse")]
    #[test]
    fn extract_scope_quoted() {
        let header = r#"Bearer error="insufficient_scope", scope="files:read files:write""#;
        assert_eq!(
            extract_scope_from_header(header),
            Some("files:read files:write".to_string())
        );
    }

    #[cfg(feature = "client-side-sse")]
    #[test]
    fn extract_scope_unquoted() {
        let header = r#"Bearer scope=read:data, error="insufficient_scope""#;
        assert_eq!(
            extract_scope_from_header(header),
            Some("read:data".to_string())
        );
    }

    #[cfg(feature = "client-side-sse")]
    #[test]
    fn extract_scope_missing() {
        let header = r#"Bearer error="invalid_token""#;
        assert_eq!(extract_scope_from_header(header), None);
    }

    #[cfg(feature = "client-side-sse")]
    #[test]
    fn extract_scope_empty_header() {
        assert_eq!(extract_scope_from_header("Bearer"), None);
    }

    #[cfg(feature = "client-side-sse")]
    #[test]
    fn validate_rejects_reserved_accept() {
        let name = http::HeaderName::from_static("accept");
        assert!(validate_custom_header(&name).is_err());
    }

    #[cfg(feature = "client-side-sse")]
    #[test]
    fn validate_rejects_reserved_session_id() {
        let name = http::HeaderName::from_static("mcp-session-id");
        assert!(validate_custom_header(&name).is_err());
    }

    #[cfg(feature = "client-side-sse")]
    #[test]
    fn validate_allows_mcp_protocol_version() {
        let name = http::HeaderName::from_static("mcp-protocol-version");
        assert!(validate_custom_header(&name).is_ok());
    }

    #[cfg(feature = "client-side-sse")]
    #[test]
    fn validate_allows_custom_header() {
        let name = http::HeaderName::from_static("x-custom");
        assert!(validate_custom_header(&name).is_ok());
    }

    #[cfg(any(feature = "client-side-sse", feature = "server-side-http"))]
    #[test]
    fn header_name_constants_match_case_insensitively() {
        let cases = [
            (HEADER_NAME_SESSION_ID, "McP-SeSsIoN-Id"),
            (HEADER_NAME_LAST_EVENT_ID, "LaSt-EvEnT-Id"),
            (HEADER_NAME_MCP_PROTOCOL_VERSION, "McP-PrOtOcOl-VeRsIoN"),
            (HEADER_NAME_MCP_METHOD, "McP-MeThOd"),
            (HEADER_NAME_MCP_NAME, "McP-NaMe"),
        ];

        for (constant, mixed_case) in cases {
            let mut headers = HeaderMap::new();
            headers.insert(
                HeaderName::from_bytes(mixed_case.as_bytes()).expect("valid header name"),
                HeaderValue::from_static("value"),
            );

            assert_eq!(
                headers.get(constant),
                Some(&HeaderValue::from_static("value"))
            );
        }
    }

    #[cfg(any(feature = "client-side-sse", feature = "server-side-http"))]
    #[test]
    fn mcp_param_lower_prefix_matches_header_names() {
        let name = HeaderName::from_static("mcp-param-user");

        assert!(name.as_str().starts_with(HEADER_MCP_PARAM_PREFIX_LOWER));
    }
}
