//! Phase 0: the MCP 2026-07-28 era-detection matrix for stdio-like transports,
//! encoded as tests before any refactor.
//!
//! Every test here cites the normative requirement it enforces. See
//! `docs/discovery-startup-compatibility.md` Part 1 for the quoted spec text and
//! the fact IDs (F3-F8) referenced below.
//!
//! Source: <https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio#backward-compatibility>
//!
//! > The server returns any other error, or does not respond within a reasonable
//! > timeout: the server is legacy. Fall back to the `initialize` handshake.
//! >
//! > The fallback MUST NOT be keyed to one specific error code: legacy servers
//! > respond to unknown pre-`initialize` requests with implementation-defined
//! > errors (commonly `-32601` or `-32602`) or not at all.
#![cfg(all(feature = "client", feature = "server", not(feature = "local")))]

use std::time::Duration;

use rmcp::{
    ClientHandler, ClientLifecycleMode, ClientServiceExt,
    model::{
        ClientJsonRpcMessage, ClientRequest, DiscoverResult, ErrorCode, ErrorData, Implementation,
        InitializeResult, ProtocolVersion, ServerCapabilities, ServerJsonRpcMessage, ServerResult,
    },
    service::ClientInitializeError,
    transport::{IntoTransport, Transport},
};

/// Bound on any single connect attempt, so a regression that hangs startup fails
/// the test instead of stalling the suite.
///
/// Deliberately larger than [`rmcp::service::DISCOVER_STARTUP_TIMEOUT`]: tests
/// that must observe the probe timeout run on a paused clock, where crossing it
/// costs no real time but a genuine hang still trips this budget.
const CONNECT_BUDGET: Duration = Duration::from_secs(120);

#[derive(Clone, Default)]
struct MatrixClient;

impl ClientHandler for MatrixClient {}

fn auto_mode() -> ClientLifecycleMode {
    ClientLifecycleMode::Auto {
        preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        legacy_version: Some(ProtocolVersion::V_2025_11_25),
    }
}

/// Drive a full `Auto` startup where the server answers the `server/discover`
/// probe with `error`, then (if the client falls back, per F3/F4) completes a
/// legacy `initialize` handshake.
///
/// Returns `Ok(true)` when the client fell back and the legacy handshake
/// completed, `Ok(false)` when the client stayed modern and surfaced the probe
/// error, or `Err` when the connect failed for another reason.
async fn probe_error_falls_back(error: ErrorData) -> Result<bool, ClientInitializeError> {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let mut server = IntoTransport::<rmcp::RoleServer, _, _>::into_transport(server_transport);

    let server_task = tokio::spawn(async move {
        let ClientJsonRpcMessage::Request(discover) =
            server.receive().await.expect("expected discover probe")
        else {
            panic!("expected the discover probe to be a request");
        };
        assert!(
            matches!(discover.request, ClientRequest::DiscoverRequest(_)),
            "startup must probe with server/discover before anything else"
        );
        server
            .send(ServerJsonRpcMessage::error(error, Some(discover.id)))
            .await
            .expect("send probe error");

        // A legacy server now expects `initialize`. If the client instead treats
        // the probe error as modern evidence it will send nothing further and
        // this receive resolves to `None`.
        match server.receive().await {
            Some(ClientJsonRpcMessage::Request(initialize))
                if matches!(initialize.request, ClientRequest::InitializeRequest(_)) =>
            {
                server
                    .send(ServerJsonRpcMessage::response(
                        ServerResult::InitializeResult(InitializeResult::new(
                            ServerCapabilities::default(),
                        )),
                        initialize.id,
                    ))
                    .await
                    .expect("send initialize response");
                // F3 fallback ordering: `initialize` is followed by
                // `notifications/initialized`.
                assert!(
                    matches!(
                        server.receive().await,
                        Some(ClientJsonRpcMessage::Notification(_))
                    ),
                    "fallback must send notifications/initialized after initialize"
                );
                true
            }
            _ => false,
        }
    });

    let connect = tokio::time::timeout(
        CONNECT_BUDGET,
        MatrixClient.serve_with_lifecycle(client_transport, auto_mode()),
    );

    match connect.await.expect("connect must not hang") {
        Ok(client) => {
            client.cancel().await.expect("cancel client");
            let fell_back = server_task.await.expect("server task");
            assert!(
                fell_back,
                "a successful Auto connect against a legacy server must have fallen back"
            );
            Ok(true)
        }
        Err(error) => {
            server_task.abort();
            Err(error)
        }
    }
}

// ---------------------------------------------------------------------------
// F4: fallback MUST NOT be keyed to one specific error code.
// ---------------------------------------------------------------------------

/// The one code that already worked before this effort. Guards against
/// regression while the surrounding logic is replaced.
#[tokio::test]
async fn method_not_found_falls_back() {
    let fell_back = probe_error_falls_back(ErrorData::new(
        ErrorCode::METHOD_NOT_FOUND,
        "Method not found",
        None,
    ))
    .await
    .expect("-32601 must fall back to the legacy handshake");
    assert!(fell_back);
}

/// The spec names `-32602` alongside `-32601` as a *common* legacy response.
/// Keying fallback to `METHOD_NOT_FOUND` alone (F12) fails this.
#[tokio::test]
async fn invalid_params_falls_back() {
    let fell_back = probe_error_falls_back(ErrorData::new(
        ErrorCode::INVALID_PARAMS,
        "Invalid params",
        None,
    ))
    .await
    .expect("-32602 must fall back: the spec names it as a common legacy response");
    assert!(fell_back);
}

/// "implementation-defined errors" — an arbitrary server-defined code is still
/// legacy evidence.
#[tokio::test]
async fn arbitrary_server_error_falls_back() {
    let fell_back = probe_error_falls_back(ErrorData::new(ErrorCode(-32000), "Server error", None))
        .await
        .expect("an unrecognized code must fall back, not fail the connect");
    assert!(fell_back);
}

/// Internal error carries no modern meaning either.
#[tokio::test]
async fn internal_error_falls_back() {
    let fell_back = probe_error_falls_back(ErrorData::new(
        ErrorCode::INTERNAL_ERROR,
        "Internal error",
        None,
    ))
    .await
    .expect("-32603 must fall back");
    assert!(fell_back);
}

// ---------------------------------------------------------------------------
// F8: the recognized-modern set. These prove the peer is modern and MUST NOT
// trigger a legacy `initialize`.
// ---------------------------------------------------------------------------

/// `MissingRequiredClientCapabilityError`, named by the streamable-HTTP
/// backward-compatibility text as a modern-server 400.
#[tokio::test]
async fn missing_required_client_capability_does_not_fall_back() {
    let outcome = probe_error_falls_back(ErrorData::new(
        ErrorCode::MISSING_REQUIRED_CLIENT_CAPABILITY,
        "Missing required client capability",
        None,
    ))
    .await;

    match outcome {
        Ok(true) => panic!("-32021 is modern evidence and must not fall back to initialize"),
        Ok(false) => panic!("connect unexpectedly succeeded without a handshake"),
        Err(error) => assert!(
            matches!(&error, ClientInitializeError::JsonRpcError(data)
                if data.code == ErrorCode::MISSING_REQUIRED_CLIENT_CAPABILITY),
            "-32021 must surface as its own error, got {error:?}"
        ),
    }
}

/// "header-validation failures" — `HEADER_MISMATCH` identifies a modern server.
#[tokio::test]
async fn header_mismatch_does_not_fall_back() {
    let outcome = probe_error_falls_back(ErrorData::new(
        ErrorCode::HEADER_MISMATCH,
        "Header mismatch",
        None,
    ))
    .await;

    match outcome {
        Ok(true) => panic!("-32020 is modern evidence and must not fall back to initialize"),
        Ok(false) => panic!("connect unexpectedly succeeded without a handshake"),
        Err(error) => assert!(
            matches!(&error, ClientInitializeError::JsonRpcError(data)
                if data.code == ErrorCode::HEADER_MISMATCH),
            "-32020 must surface as its own error, got {error:?}"
        ),
    }
}

/// `-32022` with no mutually supported version is a real incompatibility, not a
/// legacy signal.
#[tokio::test]
async fn unsupported_version_without_overlap_does_not_fall_back() {
    let server_only: ProtocolVersion =
        serde_json::from_value(serde_json::json!("2099-01-01")).unwrap();
    let outcome = probe_error_falls_back(ErrorData::unsupported_protocol_version(
        ProtocolVersion::V_2026_07_28,
        &[server_only],
    ))
    .await;

    match outcome {
        Ok(true) => panic!("-32022 must never fall back to initialize"),
        Ok(false) => panic!("connect unexpectedly succeeded without a handshake"),
        Err(error) => assert!(
            matches!(
                error,
                ClientInitializeError::NoCompatibleProtocolVersion { .. }
            ),
            "a disjoint modern version list must surface NoCompatibleProtocolVersion, got {error:?}"
        ),
    }
}

// ---------------------------------------------------------------------------
// F5: "or does not respond within a reasonable timeout" / "or not at all".
// ---------------------------------------------------------------------------

/// A legacy server that silently ignores the unknown probe but keeps the stream
/// open. Per F5 this is legacy evidence: the startup timeout must expire and the
/// client must fall back to `initialize`.
///
/// Runs on a paused clock, so crossing
/// [`rmcp::service::DISCOVER_STARTUP_TIMEOUT`] costs no wall-clock time. Auto-advance
/// moves the clock only when every task is idle, which is exactly the state this
/// models: the client is blocked on the probe and the server will never answer.
#[tokio::test(start_paused = true)]
async fn silent_probe_falls_back_after_startup_timeout() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let mut server = IntoTransport::<rmcp::RoleServer, _, _>::into_transport(server_transport);

    let server_task = tokio::spawn(async move {
        let ClientJsonRpcMessage::Request(discover) =
            server.receive().await.expect("expected discover probe")
        else {
            panic!("expected the discover probe to be a request");
        };
        assert!(matches!(
            discover.request,
            ClientRequest::DiscoverRequest(_)
        ));
        // Deliberately no reply: model a legacy server that drops the unknown
        // pre-`initialize` request but stays alive.

        match server.receive().await {
            Some(ClientJsonRpcMessage::Request(initialize))
                if matches!(initialize.request, ClientRequest::InitializeRequest(_)) =>
            {
                server
                    .send(ServerJsonRpcMessage::response(
                        ServerResult::InitializeResult(InitializeResult::new(
                            ServerCapabilities::default(),
                        )),
                        initialize.id,
                    ))
                    .await
                    .expect("send initialize response");
                // Consume `notifications/initialized` before dropping the pipe,
                // or the client's send races a BrokenPipe.
                assert!(
                    matches!(
                        server.receive().await,
                        Some(ClientJsonRpcMessage::Notification(_))
                    ),
                    "fallback must send notifications/initialized after initialize"
                );
                true
            }
            _ => false,
        }
    });

    let connect = tokio::time::timeout(
        CONNECT_BUDGET,
        MatrixClient.serve_with_lifecycle(client_transport, auto_mode()),
    );

    let client = connect
        .await
        .expect("a silent probe must time out and fall back, not hang forever")
        .expect("startup timeout must produce a legacy fallback");
    client.cancel().await.expect("cancel client");
    assert!(
        server_task.await.expect("server task"),
        "the client must send initialize after the probe timeout"
    );
}

/// EOF before any probe response. The spec's "or not at all" makes this legacy
/// evidence, so the classifier must reach a `Legacy` verdict rather than
/// surfacing the closure as a connect failure.
///
/// On a single shared stream the resulting `initialize` has nowhere to go — the
/// pipe is already gone — so the observable requirement is that the client
/// *attempted* the fallback and failed while writing it, deterministically and
/// without hanging. A transport that can re-establish the stream (the TS SDK's
/// disposable-sibling probe, Phase 5) turns this same verdict into a successful
/// connect.
#[tokio::test]
async fn eof_during_probe_is_deterministic() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let mut server = IntoTransport::<rmcp::RoleServer, _, _>::into_transport(server_transport);

    let server_task = tokio::spawn(async move {
        let ClientJsonRpcMessage::Request(discover) =
            server.receive().await.expect("expected discover probe")
        else {
            panic!("expected the discover probe to be a request");
        };
        assert!(matches!(
            discover.request,
            ClientRequest::DiscoverRequest(_)
        ));
        // Model rmcp's own server, which rejects any non-ping pre-`initialize`
        // request and terminates (F13).
        drop(server);
    });

    let connect = tokio::time::timeout(
        CONNECT_BUDGET,
        MatrixClient.serve_with_lifecycle(client_transport, auto_mode()),
    );

    let outcome = connect
        .await
        .expect("EOF during the probe must not hang the connect");
    server_task.await.expect("server task");

    match outcome {
        Ok(client) => {
            client.cancel().await.ok();
        }
        Err(error) => {
            assert!(
                !matches!(error, ClientInitializeError::Cancelled),
                "EOF must not be reported as cancellation, got {error:?}"
            );
            // The classifier reached `Legacy` and the lifecycle tried to write
            // `initialize` onto the dead stream. A transport error from that
            // write is the proof that fallback was attempted; a JSON-RPC or
            // version-negotiation error would mean EOF was misread as a
            // protocol-level outcome.
            assert!(
                matches!(error, ClientInitializeError::TransportError { .. }),
                "EOF must be classified as legacy and fail while sending the \
                 fallback initialize, got {error:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// F3: a successful probe stays modern.
// ---------------------------------------------------------------------------

/// The happy path: `DiscoverResult` means modern, and `initialize` must never
/// be sent.
#[tokio::test]
async fn successful_discovery_never_sends_initialize() {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let mut server = IntoTransport::<rmcp::RoleServer, _, _>::into_transport(server_transport);

    let server_task = tokio::spawn(async move {
        let ClientJsonRpcMessage::Request(discover) =
            server.receive().await.expect("expected discover probe")
        else {
            panic!("expected the discover probe to be a request");
        };
        server
            .send(ServerJsonRpcMessage::response(
                ServerResult::DiscoverResult(
                    DiscoverResult::new(
                        vec![ProtocolVersion::V_2026_07_28],
                        ServerCapabilities::default(),
                    )
                    .with_server_info(Implementation::new("modern-server", "1.0.0")),
                ),
                discover.id,
            ))
            .await
            .expect("send discover result");

        // Anything the client sends next must not be an `initialize`.
        if let Some(ClientJsonRpcMessage::Request(next)) = server.receive().await {
            assert!(
                !matches!(next.request, ClientRequest::InitializeRequest(_)),
                "a modern peer must never receive initialize"
            );
        }
    });

    let client = tokio::time::timeout(
        CONNECT_BUDGET,
        MatrixClient.serve_with_lifecycle(client_transport, auto_mode()),
    )
    .await
    .expect("connect must not hang")
    .expect("a DiscoverResult must complete modern startup");

    let peer_info = client.peer_info().expect("peer info");
    assert_eq!(peer_info.protocol_version, ProtocolVersion::V_2026_07_28);
    client.cancel().await.expect("cancel client");
    server_task.await.expect("server task");
}
