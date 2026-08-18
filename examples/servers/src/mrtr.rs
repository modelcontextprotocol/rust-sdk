//! SEP-2322 Multi Round-Trip Request (MRTR) end-to-end example.
//!
//! This runs a server and a client in the same process, connected over an
//! in-memory duplex stream, to show the full MRTR flow:
//!
//! * The **server** answers `tools/call` with an [`InputRequiredResult`] instead
//!   of a final result, asking the client to elicit a value first. It stores its
//!   progress in an opaque, integrity-protected `requestState` produced by a
//!   [`RequestStateCodec`], and verifies that state when the client retries.
//! * The **client** uses the high-level [`RunningService::call_tool`] helper,
//!   which automatically fulfils the elicitation through the local
//!   [`ClientHandler`] and retries the original request. The example then repeats
//!   the call with [`RunningService::call_tool_once`] to show the manual escape
//!   hatch that returns the intermediate result without retrying.
//!
//! ## Version gating
//!
//! `InputRequiredResult` is only valid once the peers have negotiated protocol
//! version `2026-07-28` or newer. Both sides advertise that version below. If a
//! server emits an `InputRequiredResult` to an older client, the SDK turns it
//! into a protocol error instead of sending it on the wire.
//!
//! ## `requestState` is untrusted input
//!
//! The client echoes `requestState` back verbatim, so from the server's point of
//! view it is attacker-controlled. A stateless server that puts meaningful data
//! in `requestState` MUST verify it. This example uses [`RequestStateCodec`] to
//! seal and open it with an HMAC tag; tampered values are rejected.
//!
//! Run with:
//!
//! ```sh
//! cargo run -p mcp-server-examples --example servers_mrtr
//! ```

use rmcp::{
    ClientHandler, ServerHandler, ServiceExt,
    model::*,
    service::{RequestContext, RoleClient, RoleServer},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// A fixed key for this example. Real deployments should load at least 32 bytes
/// of random secret key material from a secret manager and keep it stable for
/// the lifetime of any in-flight MRTR exchange.
const REQUEST_STATE_KEY: &[u8] = b"example-request-state-signing-key-32b!";

#[derive(Debug, Serialize, Deserialize)]
struct WeatherRequestState {
    awaiting: WeatherInput,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum WeatherInput {
    City,
}

/// A server that needs a city name before it can answer a weather query.
#[derive(Clone)]
struct WeatherServer {
    codec: RequestStateCodec,
}

impl WeatherServer {
    fn new(key: impl Into<Vec<u8>>) -> Result<Self, RequestStateError> {
        Ok(Self {
            codec: RequestStateCodec::try_new(key)?,
        })
    }
}

impl WeatherServer {
    fn open_request_state(&self, sealed: &str) -> Result<WeatherRequestState, ErrorData> {
        self.codec
            .open_json(sealed)
            .map_err(|_| ErrorData::invalid_params("tampered or unknown request state", None))
    }
}

fn finish_weather_request(
    input_responses: Option<&InputResponses>,
) -> Result<CallToolResult, ErrorData> {
    let response = input_responses
        .and_then(|responses| responses.get("city"))
        .ok_or_else(|| ErrorData::invalid_params("missing city elicitation response", None))?;
    let response = ElicitResult::deserialize(response)
        .map_err(|_| ErrorData::invalid_params("invalid city elicitation response", None))?;

    let message = match response.action {
        ElicitationAction::Accept => {
            let city = response
                .content
                .as_ref()
                .and_then(|content| content["city"].as_str())
                .ok_or_else(|| {
                    ErrorData::invalid_params("accepted elicitation did not include a city", None)
                })?;
            format!("It is sunny in {city}.")
        }
        ElicitationAction::Decline => "I cannot look up the weather without a city.".into(),
        ElicitationAction::Cancel => "Weather lookup cancelled.".into(),
        _ => {
            return Err(ErrorData::invalid_params(
                "unsupported elicitation action",
                None,
            ));
        }
    };

    Ok(CallToolResult::success(vec![ContentBlock::text(message)]))
}

impl ServerHandler for WeatherServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::new(ServerCapabilities::builder().enable_tools().build());
        // MRTR requires 2026-07-28 or newer.
        info.protocol_version = ProtocolVersion::V_2026_07_28;
        info
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        match request.request_state {
            // First round: ask the client to provide a city, and remember where
            // we are by sealing our progress into `requestState`.
            None => {
                let sealed = self
                    .codec
                    .seal_json(&WeatherRequestState {
                        awaiting: WeatherInput::City,
                    })
                    .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

                let mut input_requests = InputRequests::new();
                input_requests.insert(
                    "city".to_string(),
                    InputRequest::Elicitation(ElicitRequest::new(
                        ElicitRequestParams::FormElicitationParams {
                            meta: None,
                            message: "Which city do you want the weather for?".into(),
                            requested_schema: serde_json::from_value(json!({
                                "type": "object",
                                "properties": { "city": { "type": "string" } },
                                "required": ["city"]
                            }))
                            .expect("valid schema"),
                        },
                    )),
                );

                Ok(InputRequiredResult::new(Some(input_requests), Some(sealed)).into())
            }
            // Retry round: verify the echoed state before trusting it, read the
            // elicited city, and return the final result.
            Some(sealed) => {
                let state = self.open_request_state(&sealed)?;
                match state.awaiting {
                    WeatherInput::City => finish_weather_request(request.input_responses.as_ref())
                        .map(CallToolResponse::from),
                }
            }
        }
    }
}

/// A client that fulfils elicitation requests. A real client would prompt a user.
#[derive(Clone, Default)]
struct InteractiveClient;

impl ClientHandler for InteractiveClient {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::new(
            ClientCapabilities::builder().enable_elicitation().build(),
            Implementation::new("mrtr-example-client", env!("CARGO_PKG_VERSION")),
        )
        .with_protocol_version(ProtocolVersion::V_2026_07_28)
    }

    async fn create_elicitation(
        &self,
        request: ElicitRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, ErrorData> {
        if let ElicitRequestParams::FormElicitationParams { message, .. } = &request {
            println!("  [client] server asked: {message}");
        }
        // Pretend the user typed "Paris".
        Ok(ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "city": "Paris" })))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(8192);
    let server = WeatherServer::new(REQUEST_STATE_KEY)?;

    // Spin up the server side.
    tokio::spawn(async move {
        let server = server
            .serve(server_transport)
            .await
            .expect("server should start");
        let _ = server.waiting().await;
    });

    // Connect the client (this performs the initialize handshake).
    let client = InteractiveClient.serve(client_transport).await?;

    // 1. High-level auto mode: the SDK fulfils the elicitation and retries for us.
    println!("== auto mode (call_tool) ==");
    let result = client
        .call_tool(CallToolRequestParams::new("weather"))
        .await?;
    println!(
        "  [client] final result: {}\n",
        result.content[0].as_text().unwrap().text
    );

    // 2. Manual mode: get the intermediate InputRequiredResult without retrying.
    println!("== manual mode (call_tool_once) ==");
    match client
        .call_tool_once(CallToolRequestParams::new("weather"))
        .await?
    {
        CallToolResponse::InputRequired(input_required) => {
            let requests = input_required.input_requests.unwrap_or_default();
            println!(
                "  [client] server requested {} input(s); handling them yourself is up to you.",
                requests.len()
            );
        }
        CallToolResponse::Complete(result) => {
            println!("  [client] completed immediately: {result:?}");
        }
        _ => println!("  [client] unhandled response variant"),
    }

    client.cancel().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input_responses(response: ElicitResult) -> InputResponses {
        [("city".into(), serde_json::to_value(response).unwrap())].into()
    }

    #[test]
    fn finish_weather_request_uses_city_when_elicitation_is_accepted() {
        let responses = input_responses(
            ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "city": "Paris" })),
        );

        let result = finish_weather_request(Some(&responses)).unwrap();

        assert_eq!(
            result.content[0].as_text().unwrap().text,
            "It is sunny in Paris."
        );
    }

    #[test]
    fn finish_weather_request_ignores_content_when_elicitation_is_declined() {
        let responses = input_responses(
            ElicitResult::new(ElicitationAction::Decline).with_content(json!({ "city": "Paris" })),
        );

        let result = finish_weather_request(Some(&responses)).unwrap();

        assert_eq!(
            result.content[0].as_text().unwrap().text,
            "I cannot look up the weather without a city."
        );
    }

    #[test]
    fn finish_weather_request_stops_lookup_when_elicitation_is_cancelled() {
        let responses = input_responses(ElicitResult::new(ElicitationAction::Cancel));

        let result = finish_weather_request(Some(&responses)).unwrap();

        assert_eq!(
            result.content[0].as_text().unwrap().text,
            "Weather lookup cancelled."
        );
    }

    #[test]
    fn finish_weather_request_rejects_accepted_elicitation_without_city() {
        let responses =
            input_responses(ElicitResult::new(ElicitationAction::Accept).with_content(json!({})));

        let error = finish_weather_request(Some(&responses)).unwrap_err();

        assert_eq!(
            error,
            ErrorData::invalid_params("accepted elicitation did not include a city", None)
        );
    }

    #[test]
    fn finish_weather_request_rejects_missing_city_response() {
        let error = finish_weather_request(None).unwrap_err();

        assert_eq!(
            error,
            ErrorData::invalid_params("missing city elicitation response", None)
        );
    }

    #[test]
    fn open_request_state_rejects_unknown_phase() {
        let server = WeatherServer::new(REQUEST_STATE_KEY).unwrap();
        let sealed = server
            .codec
            .seal_json(&json!({ "awaiting": "country" }))
            .unwrap();

        let error = server.open_request_state(&sealed).unwrap_err();

        assert_eq!(
            error,
            ErrorData::invalid_params("tampered or unknown request state", None)
        );
    }
}
