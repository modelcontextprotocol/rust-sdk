pub const HEADER_SESSION_ID: &str = "Mcp-Session-Id";
pub const HEADER_LAST_EVENT_ID: &str = "Last-Event-Id";
pub const HEADER_MCP_PROTOCOL_VERSION: &str = "MCP-Protocol-Version";
pub const EVENT_STREAM_MIME_TYPE: &str = "text/event-stream";
pub const JSON_MIME_TYPE: &str = "application/json";

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
    if RESERVED_HEADERS
        .iter()
        .any(|&r| name.as_str().eq_ignore_ascii_case(r))
    {
        if name
            .as_str()
            .eq_ignore_ascii_case(HEADER_MCP_PROTOCOL_VERSION)
        {
            return Ok(());
        }
        return Err(name.to_string());
    }
    Ok(())
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
}
