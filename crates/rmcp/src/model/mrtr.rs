//! Multi Round-Trip Request (MRTR) types for SEP-2322.
//!
//! Provides [`InputRequiredResult`], [`InputRequests`], and [`InputResponses`]
//! for the stateless multi round-trip request pattern defined in the MCP spec.
//! [`ResultType`] lives in the parent [`super`] module alongside other base result types.
//!
//! # Overview
//!
//! A server may respond to `tools/call`, `prompts/get`, or `resources/read` with an
//! [`InputRequiredResult`] instead of the normal result. The client fulfills the
//! [`InputRequests`], then retries the original request with [`InputResponses`] and
//! the echoed `requestState`.
//!
//! # Using MRTR
//!
//! **Server:** return an [`InputRequiredResult`] from a tool/prompt/resource
//! handler via the matching outcome enum ([`CallToolResponse`],
//! [`GetPromptResponse`], [`ReadResourceResponse`]). The SDK only lets an
//! `InputRequiredResult` reach a peer that negotiated protocol version
//! `2026-07-28` or newer; older peers get a protocol error instead.
//!
//! **Client:** the high-level `RunningService` helpers — `call_tool`,
//! `get_prompt`, and `read_resource` — automatically fulfil each
//! [`InputRequest`] through the local `ClientHandler` and retry, up to
//! [`DEFAULT_MRTR_MAX_ROUNDS`]. Use the `*_once` variants (e.g.
//! `call_tool_once`) to receive an [`InputRequiredResult`] directly and drive
//! the rounds yourself.
//!
//! # `requestState` is untrusted
//!
//! The client echoes `requestState` back verbatim, so a stateless server that
//! stores meaningful data in it MUST verify integrity before trusting the echoed
//! value. Enable the `request-state` feature and use `RequestStateCodec` to seal
//! and open it, or keep the state server-side and use `requestState` only as an
//! opaque handle.
//!
//! A complete runnable walkthrough lives in the `servers_mrtr` example.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    CallToolResult, CreateMessageRequest, ElicitRequest, GetPromptResult, ListRootsRequest, Meta,
    ReadResourceResult, ResultType, ServerResult,
};

/// Default maximum number of MRTR rounds a high-level client call will drive.
///
/// This matches the default used by other Tier 1 SDKs and prevents a
/// misbehaving peer from keeping a request alive indefinitely.
pub const DEFAULT_MRTR_MAX_ROUNDS: usize = 10;

/// A server-initiated request that can appear inside [`InputRequests`].
///
/// Per the MCP spec, only `CreateMessageRequest` (sampling),
/// `ElicitRequest` (elicitation), and `ListRootsRequest` (roots)
/// are allowed. This is modeled as an untagged enum rather than a
/// `ServerRequest` alias to prevent `PingRequest` or `CustomRequest` from
/// being included.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub enum InputRequest {
    /// A `sampling/createMessage` request.
    CreateMessage(CreateMessageRequest),
    /// An `elicitation/create` request.
    Elicitation(ElicitRequest),
    /// A `roots/list` request.
    ListRoots(ListRootsRequest),
}

/// A map of server-initiated requests that the client must fulfill.
///
/// Keys are server-assigned string identifiers; values are request objects
/// (`ElicitRequest`, `CreateMessageRequest`, or `ListRootsRequest`).
pub type InputRequests = BTreeMap<String, InputRequest>;

/// A map of client responses to server-initiated requests.
///
/// Keys correspond to the keys in the [`InputRequests`] map; values are the
/// client's result for each request (`ElicitResult`, `CreateMessageResult`,
/// or `ListRootsResult`), represented as opaque JSON because the
/// heterogeneous `ClientResult` union does not derive the traits required
/// for use as a `BTreeMap` value.
pub type InputResponses = BTreeMap<String, Value>;

/// Result of a `tools/call` request, including the MRTR intermediate result.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum CallToolResponse {
    /// The server completed the tool call.
    Complete(CallToolResult),
    /// The server requires client-side input before the tool call can complete.
    InputRequired(InputRequiredResult),
}

impl From<CallToolResult> for CallToolResponse {
    fn from(result: CallToolResult) -> Self {
        Self::Complete(result)
    }
}

impl From<InputRequiredResult> for CallToolResponse {
    fn from(result: InputRequiredResult) -> Self {
        Self::InputRequired(result)
    }
}

impl From<CallToolResponse> for ServerResult {
    fn from(response: CallToolResponse) -> Self {
        match response {
            CallToolResponse::Complete(result) => ServerResult::CallToolResult(result),
            CallToolResponse::InputRequired(result) => ServerResult::InputRequiredResult(result),
        }
    }
}

/// Result of a `prompts/get` request, including the MRTR intermediate result.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum GetPromptResponse {
    /// The server completed the prompt request.
    Complete(GetPromptResult),
    /// The server requires client-side input before the prompt can be returned.
    InputRequired(InputRequiredResult),
}

impl From<GetPromptResult> for GetPromptResponse {
    fn from(result: GetPromptResult) -> Self {
        Self::Complete(result)
    }
}

impl From<InputRequiredResult> for GetPromptResponse {
    fn from(result: InputRequiredResult) -> Self {
        Self::InputRequired(result)
    }
}

impl From<GetPromptResponse> for ServerResult {
    fn from(response: GetPromptResponse) -> Self {
        match response {
            GetPromptResponse::Complete(result) => ServerResult::GetPromptResult(result),
            GetPromptResponse::InputRequired(result) => ServerResult::InputRequiredResult(result),
        }
    }
}

/// Result of a `resources/read` request, including the MRTR intermediate result.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ReadResourceResponse {
    /// The server completed the resource read.
    Complete(ReadResourceResult),
    /// The server requires client-side input before the resource can be returned.
    InputRequired(InputRequiredResult),
}

impl From<ReadResourceResult> for ReadResourceResponse {
    fn from(result: ReadResourceResult) -> Self {
        Self::Complete(result)
    }
}

impl From<InputRequiredResult> for ReadResourceResponse {
    fn from(result: InputRequiredResult) -> Self {
        Self::InputRequired(result)
    }
}

impl From<ReadResourceResponse> for ServerResult {
    fn from(response: ReadResourceResponse) -> Self {
        match response {
            ReadResourceResponse::Complete(result) => ServerResult::ReadResourceResult(result),
            ReadResourceResponse::InputRequired(result) => {
                ServerResult::InputRequiredResult(result)
            }
        }
    }
}

/// A result indicating that additional input is needed before the request
/// can be completed.
///
/// At least one of [`input_requests`](Self::input_requests) or
/// [`request_state`](Self::request_state) MUST be present.
///
/// Servers MAY send this in response to `tools/call`, `prompts/get`, or
/// `resources/read`. Servers MUST NOT send this for any other request.
///
/// # Examples
///
/// ```
/// use rmcp::model::InputRequiredResult;
///
/// let result = InputRequiredResult::from_request_state("opaque-server-state");
/// assert!(result.input_requests.is_none());
/// assert_eq!(result.request_state.as_deref(), Some("opaque-server-state"));
/// ```
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct InputRequiredResult {
    /// Always `"input_required"` for this result type.
    pub result_type: ResultType,

    /// Server-initiated requests that the client must fulfill before retrying.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_requests: Option<InputRequests>,

    /// Opaque request state to be echoed back by the client on retry.
    /// Clients MUST NOT inspect, parse, modify, or make any assumptions
    /// about the contents.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_state: Option<String>,

    /// Optional protocol-level metadata.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

/// Custom deserializer that requires `resultType: "input_required"` to prevent
/// greedy matching in the untagged `ServerResult` enum (which would otherwise
/// swallow empty objects or unknown shapes).
impl<'de> Deserialize<'de> for InputRequiredResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Helper {
            result_type: Option<ResultType>,
            input_requests: Option<InputRequests>,
            request_state: Option<String>,
            #[serde(rename = "_meta")]
            meta: Option<Meta>,
        }

        let helper = Helper::deserialize(deserializer)?;

        match &helper.result_type {
            Some(rt) if rt.is_input_required() => {}
            _ => {
                return Err(serde::de::Error::custom(
                    "InputRequiredResult requires resultType to be \"input_required\"",
                ));
            }
        }

        Ok(InputRequiredResult {
            result_type: ResultType::INPUT_REQUIRED,
            input_requests: helper.input_requests,
            request_state: helper.request_state,
            meta: helper.meta,
        })
    }
}

impl InputRequiredResult {
    /// Creates a new `InputRequiredResult` with both input requests and request state.
    pub fn new(input_requests: Option<InputRequests>, request_state: Option<String>) -> Self {
        Self {
            result_type: ResultType::INPUT_REQUIRED,
            input_requests,
            request_state,
            meta: None,
        }
    }

    /// Creates from input requests only.
    pub fn from_input_requests(input_requests: InputRequests) -> Self {
        Self::new(Some(input_requests), None)
    }

    /// Creates from request state only (e.g. for load shedding).
    pub fn from_request_state(request_state: impl Into<String>) -> Self {
        Self::new(None, Some(request_state.into()))
    }

    /// Sets optional metadata.
    pub fn with_meta(mut self, meta: Meta) -> Self {
        self.meta = Some(meta);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod result_type {
        use super::*;

        #[test]
        fn default_is_complete() {
            assert_eq!(ResultType::default(), ResultType::COMPLETE);
        }

        #[test]
        fn serializes_complete() {
            assert_eq!(
                serde_json::to_value(&ResultType::COMPLETE).unwrap(),
                serde_json::json!("complete")
            );
        }

        #[test]
        fn serializes_input_required() {
            assert_eq!(
                serde_json::to_value(&ResultType::INPUT_REQUIRED).unwrap(),
                serde_json::json!("input_required")
            );
        }

        #[test]
        fn deserializes_known_values() {
            let complete: ResultType =
                serde_json::from_value(serde_json::json!("complete")).unwrap();
            assert_eq!(complete, ResultType::COMPLETE);

            let input_required: ResultType =
                serde_json::from_value(serde_json::json!("input_required")).unwrap();
            assert_eq!(input_required, ResultType::INPUT_REQUIRED);
        }

        #[test]
        fn preserves_unknown_extension_values() {
            let custom: ResultType =
                serde_json::from_value(serde_json::json!("streaming")).unwrap();
            assert_eq!(custom.as_str(), "streaming");
            assert!(!custom.is_complete());
            assert!(!custom.is_input_required());

            let reserialized = serde_json::to_value(&custom).unwrap();
            assert_eq!(reserialized, serde_json::json!("streaming"));
        }
    }

    mod input_required_result {
        use super::*;

        #[test]
        fn deserializes_with_requests_and_state() {
            let json = serde_json::json!({
                "resultType": "input_required",
                "inputRequests": {
                    "github_login": {
                        "method": "elicitation/create",
                        "params": {
                            "message": "Please provide your GitHub username",
                            "requestedSchema": {
                                "type": "object",
                                "properties": { "name": { "type": "string" } },
                                "required": ["name"]
                            }
                        }
                    },
                    "capital_of_france": {
                        "method": "sampling/createMessage",
                        "params": {
                            "messages": [{
                                "role": "user",
                                "content": { "type": "text", "text": "What is the capital of France?" }
                            }],
                            "maxTokens": 100
                        }
                    }
                },
                "requestState": "eyJsb2NhdGlvbiI6Ik5ldyBZb3JrIn0"
            });

            let result: InputRequiredResult = serde_json::from_value(json).unwrap();

            let requests = result
                .input_requests
                .as_ref()
                .expect("should have input_requests");
            assert_eq!(requests.len(), 2);
            assert!(requests.contains_key("github_login"));
            assert!(requests.contains_key("capital_of_france"));
            assert_eq!(
                result.request_state.as_deref(),
                Some("eyJsb2NhdGlvbiI6Ik5ldyBZb3JrIn0")
            );
        }

        #[test]
        fn roundtrip_preserves_all_fields() {
            let json = serde_json::json!({
                "resultType": "input_required",
                "inputRequests": {
                    "key": {
                        "method": "elicitation/create",
                        "params": {
                            "message": "test",
                            "requestedSchema": { "type": "object", "properties": {} }
                        }
                    }
                },
                "requestState": "abc123"
            });

            let result: InputRequiredResult = serde_json::from_value(json).unwrap();
            let reserialized = serde_json::to_value(&result).unwrap();

            assert_eq!(reserialized["resultType"], "input_required");
            assert!(reserialized["inputRequests"].is_object());
            assert_eq!(reserialized["requestState"], "abc123");
        }

        #[test]
        fn deserializes_with_request_state_only() {
            let json = serde_json::json!({
                "resultType": "input_required",
                "requestState": "eyJwcm9ncmVzcyI6IjUwJSJ9"
            });

            let result: InputRequiredResult = serde_json::from_value(json).unwrap();

            assert!(result.input_requests.is_none());
            assert_eq!(
                result.request_state.as_deref(),
                Some("eyJwcm9ncmVzcyI6IjUwJSJ9")
            );
        }

        #[test]
        fn rejects_missing_result_type() {
            let json = serde_json::json!({
                "requestState": "some-state"
            });
            let err = serde_json::from_value::<InputRequiredResult>(json).unwrap_err();
            assert!(
                err.to_string().contains("input_required"),
                "error should mention the required resultType, got: {err}"
            );
        }

        #[test]
        fn rejects_wrong_result_type() {
            let json = serde_json::json!({
                "resultType": "complete",
                "requestState": "some-state"
            });
            let err = serde_json::from_value::<InputRequiredResult>(json).unwrap_err();
            assert!(
                err.to_string().contains("input_required"),
                "error should mention the required resultType, got: {err}"
            );
        }
    }

    mod input_responses {
        use super::*;

        #[test]
        fn deserializes_heterogeneous_results() {
            let json = serde_json::json!({
                "github_login": {
                    "action": "accept",
                    "content": { "name": "octocat" }
                },
                "capital_of_france": {
                    "role": "assistant",
                    "content": { "type": "text", "text": "Paris." },
                    "model": "claude-3-sonnet-20240307",
                    "stopReason": "endTurn"
                }
            });

            let responses: InputResponses = serde_json::from_value(json).unwrap();

            assert_eq!(responses.len(), 2);
            assert!(responses.contains_key("github_login"));
            assert!(responses.contains_key("capital_of_france"));
        }
    }

    mod constructors {
        use super::*;

        #[test]
        fn from_request_state_sets_state_only() {
            let result = InputRequiredResult::from_request_state("opaque");

            assert_eq!(result.result_type, ResultType::INPUT_REQUIRED);
            assert!(result.input_requests.is_none());
            assert_eq!(result.request_state.as_deref(), Some("opaque"));
        }

        #[test]
        fn from_input_requests_sets_requests_only() {
            let mut requests = InputRequests::new();
            requests.insert(
                "key".to_string(),
                serde_json::from_value(serde_json::json!({
                    "method": "elicitation/create",
                    "params": {
                        "message": "test",
                        "requestedSchema": { "type": "object", "properties": {} }
                    }
                }))
                .unwrap(),
            );

            let result = InputRequiredResult::from_input_requests(requests);

            assert!(result.input_requests.is_some());
            assert!(result.request_state.is_none());
        }
    }
}
