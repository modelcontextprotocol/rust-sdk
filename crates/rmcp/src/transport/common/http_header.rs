pub const HEADER_SESSION_ID: &str = "Mcp-Session-Id";
pub const HEADER_LAST_EVENT_ID: &str = "Last-Event-Id";
pub const HEADER_MCP_PROTOCOL_VERSION: &str = "MCP-Protocol-Version";
pub const EVENT_STREAM_MIME_TYPE: &str = "text/event-stream";
pub const JSON_MIME_TYPE: &str = "application/json";

// SEP-2243 standard headers, gated on protocol version >= 2026-07-28.
pub const HEADER_MCP_METHOD: &str = "Mcp-Method";
pub const HEADER_MCP_NAME: &str = "Mcp-Name";
pub const HEADER_MCP_PARAM_PREFIX: &str = "Mcp-Param-";

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

/// Optional whitespace between HTTP list members (RFC 9110 §5.6.3).
#[cfg(any(feature = "auth", feature = "client-side-sse"))]
fn is_ows(ch: char) -> bool {
    ch == ' ' || ch == '\t'
}

/// A `tchar` (RFC 9110 §5.6.2): any visible ASCII character except delimiters.
#[cfg(any(feature = "auth", feature = "client-side-sse"))]
fn is_tchar(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(ch)
}

/// Whether `s` is a `token68` (RFC 9110 §11.2): base64-style characters
/// followed by optional `=` padding, as in `Negotiate YIIB...==`.
#[cfg(any(feature = "auth", feature = "client-side-sse"))]
fn is_token68(s: &str) -> bool {
    let body = s.trim_end_matches('=');
    !body.is_empty()
        && body
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "-._~+/".contains(ch))
}

/// One challenge from a `WWW-Authenticate` field value (RFC 9110 §11.3):
/// `auth-scheme [ 1*SP ( token68 / #auth-param ) ]`.
#[cfg(any(feature = "auth", feature = "client-side-sse"))]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Challenge<'a> {
    /// The `auth-scheme` token. Compare it case-insensitively (RFC 9110 §11.1).
    pub scheme: &'a str,
    /// The challenge's `auth-param`s in field order, with quoted-string values
    /// unescaped (RFC 9110 §5.6.4). A `token68` carries no parameters.
    pub params: Vec<(&'a str, std::borrow::Cow<'a, str>)>,
}

#[cfg(any(feature = "auth", feature = "client-side-sse"))]
impl Challenge<'_> {
    /// The value of the parameter `name`, matched case-insensitively (RFC 9110 §11.2).
    ///
    /// A name may only occur once per challenge; the first occurrence wins
    /// if a server repeats it.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_ref())
    }

    /// Whether this is an RFC 6750 `Bearer` challenge.
    pub fn is_bearer(&self) -> bool {
        self.scheme.eq_ignore_ascii_case("Bearer")
    }
}

/// Splits an HTTP list (RFC 9110 §5.6.1) on the commas that are not inside a
/// quoted-string, honouring `quoted-pair` escapes (RFC 9110 §5.6.4).
///
/// `;` is accepted as a separator too. It is not in the grammar, but some
/// servers emit `realm="x"; scope="y"` and the SDK has always read past it.
#[cfg(any(feature = "auth", feature = "client-side-sse"))]
fn split_list(value: &str) -> impl Iterator<Item = &str> {
    let mut quoted = false;
    let mut escaped = false;
    value.split(move |ch| {
        if escaped {
            escaped = false;
        } else if quoted && ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            quoted = !quoted;
        } else if (ch == ',' || ch == ';') && !quoted {
            return true;
        }
        false
    })
}

/// Unescapes the body of a quoted-string that starts right after its opening
/// DQUOTE. Returns the value and the number of bytes consumed up to and
/// including the closing DQUOTE, or `None` when that DQUOTE is missing.
#[cfg(any(feature = "auth", feature = "client-side-sse"))]
fn unquote(body: &str) -> Option<(std::borrow::Cow<'_, str>, usize)> {
    if let Some(end) = body.find(['\\', '"'])
        && body.as_bytes()[end] == b'"'
    {
        return Some((std::borrow::Cow::Borrowed(&body[..end]), end + 1));
    }
    let mut value = String::new();
    let mut chars = body.char_indices();
    while let Some((index, ch)) = chars.next() {
        match ch {
            // quoted-pair: the backslash stands for the octet that follows.
            '\\' => value.push(chars.next()?.1),
            '"' => return Some((std::borrow::Cow::Owned(value), index + 1)),
            _ => value.push(ch),
        }
    }
    None
}

/// Splits `token BWS "=" BWS` off the start of `text` (RFC 9110 §11.2),
/// returning the parameter name and the text that follows the `=`.
///
/// `None` when `text` does not start with an `auth-param`, which is how a
/// challenge start is recognised: an `auth-scheme` is a bare token.
#[cfg(any(feature = "auth", feature = "client-side-sse"))]
fn split_auth_param(text: &str) -> Option<(&str, &str)> {
    let (name, value) = text.split_once('=')?;
    let name = name.trim_end_matches(is_ows);
    (!name.is_empty() && name.chars().all(is_tchar))
        .then_some((name, value.trim_start_matches(is_ows)))
}

/// Parses an `auth-param` value, `token / quoted-string` (RFC 9110 §11.2),
/// returning it with the number of bytes it occupied.
///
/// Unquoted values are read up to the first whitespace, which keeps accepting
/// unquoted URLs and `scope=read:data` the way the SDK always has. `None` for
/// an empty value or an unterminated quoted-string.
#[cfg(any(feature = "auth", feature = "client-side-sse"))]
fn parse_param_value(value: &str) -> Option<(std::borrow::Cow<'_, str>, usize)> {
    if let Some(quoted) = value.strip_prefix('"') {
        let (unquoted, consumed) = unquote(quoted)?;
        return Some((unquoted, consumed + 1));
    }
    let end = value
        .find(|ch: char| ch.is_ascii_whitespace())
        .unwrap_or(value.len());
    (end > 0).then(|| (std::borrow::Cow::Borrowed(&value[..end]), end))
}

/// Appends every `auth-param` at the start of `text` to `params`.
///
/// The grammar separates parameters with commas, which [`split_list`] has
/// already handled, so this normally reads one parameter. It keeps going when
/// another `name=value` follows after whitespace alone, since some servers
/// omit the commas and the SDK has always tolerated that. Anything else ends
/// the parameter list.
#[cfg(any(feature = "auth", feature = "client-side-sse"))]
fn push_params<'a>(mut text: &'a str, params: &mut Vec<(&'a str, std::borrow::Cow<'a, str>)>) {
    while let Some((name, rest)) = split_auth_param(text.trim_start_matches(is_ows)) {
        let Some((value, consumed)) = parse_param_value(rest) else {
            return;
        };
        params.push((name, value));
        text = &rest[consumed..];
    }
}

/// Parses the challenges in a `WWW-Authenticate` field value (RFC 9110 §11.6.1).
///
/// The value is a comma-separated list of challenges, each of which may itself
/// hold a comma-separated list of parameters. A member shaped like
/// `name=value` continues the current challenge; any other member starts a new
/// one, since an `auth-scheme` is a bare token. Repeated header fields joined
/// with `", "` (see [`www_authenticate_header`]) parse exactly like one field
/// (RFC 9110 §5.3).
#[cfg(any(feature = "auth", feature = "client-side-sse"))]
pub(crate) fn parse_challenges(header: &str) -> Vec<Challenge<'_>> {
    let mut challenges: Vec<Challenge<'_>> = Vec::new();
    for member in split_list(header) {
        let member = member.trim_matches(is_ows);
        if member.is_empty() {
            // An empty list member is harmless (RFC 9110 §11.6.1, note).
            continue;
        }
        if split_auth_param(member).is_some() {
            // Parameters of the current challenge. Ones that precede any
            // scheme are dropped.
            if let Some(current) = challenges.last_mut() {
                push_params(member, &mut current.params);
            }
            continue;
        }
        let (scheme, rest) = member.split_once(is_ows).unwrap_or((member, ""));
        let mut challenge = Challenge {
            scheme,
            params: Vec::new(),
        };
        // `rest` is either a token68, which carries nothing the SDK reads
        // and whose `=` padding must not be mistaken for a parameter, or
        // the challenge's first auth-param(s).
        let rest = rest.trim_start_matches(is_ows);
        if !is_token68(rest) {
            push_params(rest, &mut challenge.params);
        }
        challenges.push(challenge);
    }
    challenges
}

/// The `Bearer` challenges in a `WWW-Authenticate` field value, in order.
#[cfg(any(feature = "auth", feature = "client-side-sse"))]
pub(crate) fn bearer_challenges(header: &str) -> Vec<Challenge<'_>> {
    let mut challenges = parse_challenges(header);
    challenges.retain(Challenge::is_bearer);
    challenges
}

/// Every `WWW-Authenticate` field of a response, combined into one field value.
///
/// RFC 9110 §5.3 lets a recipient join repeated field lines "in order,
/// separated by a comma" without changing their meaning, and §11.6.1 warns
/// that the header "can occur multiple times". Returns `Ok(None)` when the
/// response has no such field and an error when a field value is not visible
/// ASCII, which the string-based challenge API cannot represent.
#[cfg(feature = "client-side-sse")]
pub(crate) fn www_authenticate_header(
    headers: &http::HeaderMap,
) -> Result<Option<String>, http::header::ToStrError> {
    let mut values = headers.get_all(http::header::WWW_AUTHENTICATE).iter();
    let Some(first) = values.next() else {
        return Ok(None);
    };
    let mut combined = first.to_str()?.to_owned();
    for value in values {
        combined.push_str(", ");
        combined.push_str(value.to_str()?);
    }
    Ok(Some(combined))
}

/// The `scope` a 403 `Bearer` challenge asks for (RFC 6750 §3): the first
/// `scope` parameter of any `Bearer` challenge, in field order.
///
/// Parameters of other schemes and text inside quoted values are never
/// mistaken for it. With several `Bearer` challenges the first `scope` wins,
/// the same first-occurrence rule as
/// [`WWWAuthenticateParams::parse`](crate::transport::auth::WWWAuthenticateParams::parse)
/// and the TypeScript and Python SDKs.
#[cfg(feature = "client-side-sse")]
pub(crate) fn extract_scope_from_header(header: &str) -> Option<String> {
    bearer_challenges(header)
        .iter()
        .find_map(|challenge| challenge.param("scope"))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    #[cfg(any(feature = "auth", feature = "client-side-sse"))]
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
    #[rstest::rstest]
    #[case::other_scheme_first(
        r#"Basic realm="proxy", scope="basic", Bearer error="insufficient_scope", scope="mcp:read""#,
        Some("mcp:read")
    )]
    #[case::only_other_scheme(r#"Basic realm="proxy", scope="basic""#, None)]
    #[case::first_bearer_scope_wins(
        r#"Bearer realm="a", scope="read", Bearer error="insufficient_scope", scope="write""#,
        Some("read")
    )]
    #[case::later_bearer_challenge(
        r#"Bearer error="invalid_token", Bearer realm="b", scope="read""#,
        Some("read")
    )]
    #[case::semicolon_separator(r#"Bearer realm="x"; scope="y""#, Some("y"))]
    #[case::missing_commas(r#"Bearer error="insufficient_scope" scope="x""#, Some("x"))]
    #[case::ignores_quoted_text(
        r#"Basic realm="scope=wrong", Bearer error_description="scope=wrong, error=wrong", scope="right""#,
        Some("right")
    )]
    #[case::case_insensitive(r#"bEaReR ScOpE = "read""#, Some("read"))]
    #[case::escaped_quote(r#"Bearer scope="read\"write""#, Some(r#"read"write"#))]
    #[case::unterminated_quote(r#"Bearer scope="read"#, None)]
    #[case::junk_after_closing_quote(r#"Bearer scope="read"junk, realm="x""#, Some("read"))]
    fn extract_scope_keeps_challenges_apart(#[case] header: &str, #[case] expected: Option<&str>) {
        assert_eq!(extract_scope_from_header(header).as_deref(), expected);
    }

    #[cfg(feature = "client-side-sse")]
    #[test]
    fn www_authenticate_header_joins_repeated_fields_in_order() {
        let mut headers = http::HeaderMap::new();
        assert_eq!(www_authenticate_header(&headers).unwrap(), None);
        headers.append(
            http::header::WWW_AUTHENTICATE,
            http::HeaderValue::from_static(r#"Basic realm="proxy""#),
        );
        assert_eq!(
            www_authenticate_header(&headers).unwrap().as_deref(),
            Some(r#"Basic realm="proxy""#)
        );
        headers.append(
            http::header::WWW_AUTHENTICATE,
            http::HeaderValue::from_static(r#"Bearer resource_metadata="/prm", scope="read""#),
        );
        assert_eq!(
            www_authenticate_header(&headers).unwrap().as_deref(),
            Some(r#"Basic realm="proxy", Bearer resource_metadata="/prm", scope="read""#)
        );
    }

    #[cfg(feature = "client-side-sse")]
    #[test]
    fn www_authenticate_header_rejects_non_ascii_field_values() {
        let mut headers = http::HeaderMap::new();
        headers.append(
            http::header::WWW_AUTHENTICATE,
            http::HeaderValue::from_static("Bearer"),
        );
        headers.append(
            http::header::WWW_AUTHENTICATE,
            http::HeaderValue::from_bytes(b"Basic realm=\"\xff\"").unwrap(),
        );
        assert!(www_authenticate_header(&headers).is_err());
    }

    /// RFC 9110 §11.6.1's own example: two challenges, the second carrying
    /// three parameters, one with an escaped quote.
    #[cfg(any(feature = "auth", feature = "client-side-sse"))]
    #[test]
    fn parse_challenges_rfc9110_example() {
        let challenges = parse_challenges(
            r#"Basic realm="simple", Newauth realm="apps", type=1, title="Login to \"apps\"""#,
        );
        assert_eq!(challenges.len(), 2);
        assert_eq!(challenges[0].scheme, "Basic");
        assert_eq!(challenges[0].param("realm"), Some("simple"));
        assert_eq!(challenges[0].params.len(), 1);
        assert_eq!(challenges[1].scheme, "Newauth");
        assert_eq!(challenges[1].param("realm"), Some("apps"));
        assert_eq!(challenges[1].param("type"), Some("1"));
        assert_eq!(challenges[1].param("title"), Some(r#"Login to "apps""#));
        assert_eq!(challenges[1].params.len(), 3);
    }

    #[cfg(any(feature = "auth", feature = "client-side-sse"))]
    #[rstest::rstest]
    #[case::bare_scheme("Bearer", vec![("Bearer", vec![])])]
    #[case::empty("", vec![])]
    #[case::empty_members(", ,", vec![])]
    #[case::token68_is_not_a_parameter(
        "Negotiate YIIBase64==, Bearer realm=\"x\"",
        vec![("Negotiate", vec![]), ("Bearer", vec![("realm", "x")])]
    )]
    #[case::bad_whitespace_around_equals(
        "Bearer realm = \"x\" ,\tscope =read",
        vec![("Bearer", vec![("realm", "x"), ("scope", "read")])]
    )]
    #[case::quoted_comma_and_scheme_names(
        r#"Basic realm="a, Bearer scope=b", Bearer scope="c""#,
        vec![("Basic", vec![("realm", "a, Bearer scope=b")]), ("Bearer", vec![("scope", "c")])]
    )]
    #[case::unterminated_quoted_value_is_dropped(
        r#"Bearer scope="read"#,
        vec![("Bearer", vec![])]
    )]
    #[case::token68_padding_then_params(
        "Bearer abc=, Bearer scope=read",
        vec![("Bearer", vec![]), ("Bearer", vec![("scope", "read")])]
    )]
    #[case::parameter_before_any_scheme_is_dropped(
        r#"realm="x", Bearer scope="read""#,
        vec![("Bearer", vec![("scope", "read")])]
    )]
    #[case::unquoted_url_and_semicolon_separator(
        r#"Bearer resource_metadata=https://example.com/prm; error="invalid_token", scope=read:data"#,
        vec![("Bearer", vec![
            ("resource_metadata", "https://example.com/prm"),
            ("error", "invalid_token"),
            ("scope", "read:data"),
        ])]
    )]
    #[case::missing_commas_between_parameters(
        r#"Bearer realm="a" scope="b" error=c, Basic realm="d" scope="e""#,
        vec![
            ("Bearer", vec![("realm", "a"), ("scope", "b"), ("error", "c")]),
            ("Basic", vec![("realm", "d"), ("scope", "e")]),
        ]
    )]
    #[case::junk_ends_the_parameter_list(
        r#"Bearer scope=read junk extra=1, realm="x""#,
        vec![("Bearer", vec![("scope", "read"), ("realm", "x")])]
    )]
    #[case::case_insensitive_lookup(
        r#"bearer RESOURCE_METADATA="/prm""#,
        vec![("bearer", vec![("RESOURCE_METADATA", "/prm")])]
    )]
    fn parse_challenges_follows_the_grammar(
        #[case] header: &str,
        #[case] expected: Vec<(&str, Vec<(&str, &str)>)>,
    ) {
        let challenges = parse_challenges(header);
        let actual: Vec<(&str, Vec<(&str, &str)>)> = challenges
            .iter()
            .map(|challenge| {
                (
                    challenge.scheme,
                    challenge
                        .params
                        .iter()
                        .map(|(name, value)| (*name, value.as_ref()))
                        .collect(),
                )
            })
            .collect();
        assert_eq!(actual, expected);
        if let Some(bearer) = challenges.iter().find(|c| c.is_bearer()) {
            assert_eq!(
                bearer.param("resource_metadata"),
                bearer.param("RESOURCE_METADATA")
            );
        }
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
