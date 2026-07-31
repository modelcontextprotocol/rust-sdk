//! Pure era classification for the MCP 2026-07-28 `server/discover` startup probe.
//!
//! This module answers one question: given normalized facts about a startup
//! probe, is the peer modern or initialization-era?
//!
//! It is deliberately free of I/O, async, and mutable state. All retry
//! bookkeeping lives in the caller. That keeps the specification matrix
//! (`docs/discovery-startup-compatibility.md`, Part 1) directly unit-testable.
//!
//! # Specification
//!
//! [stdio backward compatibility][stdio] defines three probe outcomes:
//!
//! > - The server returns a `DiscoverResult`: the server is modern. [...]
//! > - The server returns a recognized modern JSON-RPC error such as
//! >   `UnsupportedProtocolVersionError`: the server is modern but does not
//! >   support the requested version. [...] Do **not** fall back to `initialize`.
//! > - The server returns any other error, or does not respond within a
//! >   reasonable timeout: the server is legacy. Fall back to the `initialize`
//! >   handshake.
//! >
//! > The fallback **MUST NOT** be keyed to one specific error code: legacy
//! > servers respond to unknown pre-`initialize` requests with
//! > implementation-defined errors (commonly `-32601` or `-32602`) or not at all.
//!
//! That `MUST NOT` is why classification is a **denylist**: [`ProbeVerdict::Legacy`]
//! is the default and modern evidence is the enumerated exception. Adding a new
//! modern error code is then an explicit act, and no legacy error can be
//! overlooked.
//!
//! [stdio]: https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio#backward-compatibility

use crate::model::{ErrorCode, ErrorData, ProtocolVersion};

/// Recognized modern JSON-RPC error codes.
///
/// Enumerated by the [Streamable HTTP backward-compatibility rules][http], which
/// name `UnsupportedProtocolVersionError`,
/// `MissingRequiredClientCapabilityError`, and "header-validation failures" as
/// errors that modern servers return with HTTP 400.
///
/// A code in this set positively identifies a modern server and **must never**
/// produce a legacy fallback. Only [`ErrorCode::UNSUPPORTED_PROTOCOL_VERSION`]
/// is additionally actionable, because only it advertises the server's supported
/// versions.
///
/// [http]: https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http#backward-compatibility
pub(crate) const RECOGNIZED_MODERN_ERROR_CODES: [ErrorCode; 3] = [
    ErrorCode::UNSUPPORTED_PROTOCOL_VERSION,
    ErrorCode::MISSING_REQUIRED_CLIENT_CAPABILITY,
    ErrorCode::HEADER_MISMATCH,
];

/// Whether `code` positively identifies a modern (per-request-metadata) server.
pub(crate) fn is_recognized_modern_error(code: ErrorCode) -> bool {
    RECOGNIZED_MODERN_ERROR_CODES.contains(&code)
}

/// Normalized facts about one startup probe exchange.
///
/// Transports report *facts*; this module decides the era. A transport never
/// returns a verdict.
#[derive(Debug, Clone)]
pub(crate) enum DiscoverProbeOutcome {
    /// The peer answered with a JSON-RPC result. Carries the server's advertised
    /// versions, already extracted from the `DiscoverResult`.
    DiscoverResult {
        supported_versions: Vec<ProtocolVersion>,
    },

    /// The peer answered with a JSON-RPC result that was not a usable
    /// `DiscoverResult`.
    UnparseableResult,

    /// The peer answered with an in-band JSON-RPC error.
    RpcError(ErrorData),

    /// An HTTP-layer rejection.
    ///
    /// `status` is the only status this module branches on: per the Streamable
    /// HTTP binding, body inspection is scoped to `400 Bad Request`, so every
    /// other status surfaces unchanged.
    ///
    /// Classification is implemented and unit-tested here; the Streamable HTTP
    /// transport starts constructing this once HTTP responses preserve their
    /// status (Phase 3).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "constructed by the Streamable HTTP startup exchange in Phase 3"
        )
    )]
    HttpRejection {
        status: u16,
        jsonrpc_error: Option<ErrorData>,
    },

    /// No response arrived within the startup timeout.
    NoResponse,

    /// The stream closed before a response arrived.
    Closed,
}

/// The era decision for one probe outcome.
#[derive(Debug, Clone)]
pub(crate) enum ProbeVerdict {
    /// Modern peer; `version` was selected from its advertised list.
    Modern { version: ProtocolVersion },

    /// Modern peer that rejected the requested version but named a mutually
    /// supported one. Re-probe at `version`.
    RetryVersion { version: ProtocolVersion },

    /// Initialization-era peer: send `initialize`.
    Legacy,

    /// Not era evidence. Surface the error unchanged.
    ///
    /// `NoOverlap` is distinguished so the caller can raise
    /// `NoCompatibleProtocolVersion` with both version lists.
    Error(ProbeError),
}

/// Why a probe outcome could not settle the era.
#[derive(Debug, Clone)]
pub(crate) enum ProbeError {
    /// The peer is provably modern but shares no version with this client.
    NoOverlap {
        server_supported: Vec<ProtocolVersion>,
    },

    /// A recognized modern error that version selection cannot resolve, or an
    /// HTTP status that is not a fallback trigger.
    Rpc(ErrorData),

    /// The probe produced no response and this binding does not treat silence
    /// as era evidence.
    NoResponse,

    /// The stream closed and this binding does not treat closure as era
    /// evidence.
    Closed,
}

/// Inputs to classification, owned by the caller.
#[derive(Debug, Clone)]
pub(crate) struct ProbeContext {
    /// Modern versions this client offers, in preference order.
    pub preferred_versions: Vec<ProtocolVersion>,

    /// Versions already attempted, to bound the retry loop.
    pub attempted: Vec<ProtocolVersion>,

    /// The version this probe carried.
    pub requested_version: ProtocolVersion,

    /// Whether a legacy `initialize` fallback is permitted at all.
    ///
    /// `false` in explicit `Discover` mode, where a legacy verdict is a
    /// lifecycle error rather than a transition.
    pub fallback_available: bool,

    /// Whether absence of a response is legacy evidence.
    ///
    /// True for stdio and stdio-like byte streams, where the spec says a server
    /// that "does not respond within a reasonable timeout" is legacy. False for
    /// HTTP, where a deployed server answers and silence is an outage.
    pub silence_is_legacy: bool,
}

impl ProbeContext {
    /// The highest-preference version the client offers that also appears in
    /// `server_supported`.
    fn mutual_version(&self, server_supported: &[ProtocolVersion]) -> Option<ProtocolVersion> {
        self.preferred_versions
            .iter()
            .find(|version| server_supported.contains(version))
            .cloned()
    }

    /// The next version to probe, or `None` when the retry loop must stop.
    ///
    /// Prefers an unattempted mutual version. As a special case the *current*
    /// version may be retried once: a server that rejects version X while naming
    /// X in its own `supported` list is answering incoherently, and one re-probe
    /// resolves the common case where the rejection referred to a different
    /// field. Attempting it a second time would not terminate.
    fn next_version(&self, server_supported: &[ProtocolVersion]) -> Option<ProtocolVersion> {
        let may_retry_current = self
            .attempted
            .iter()
            .filter(|version| *version == &self.requested_version)
            .count()
            == 1;
        self.preferred_versions
            .iter()
            .find(|version| {
                server_supported.contains(version)
                    && (!self.attempted.contains(version)
                        || (may_retry_current && *version == &self.requested_version))
            })
            .cloned()
    }
}

/// Classify one startup probe outcome.
///
/// Pure: no I/O, no state mutation. See the module documentation for the
/// governing specification text.
pub(crate) fn classify_probe_outcome(
    outcome: DiscoverProbeOutcome,
    ctx: &ProbeContext,
) -> ProbeVerdict {
    match outcome {
        // A `DiscoverResult` is definitive modern evidence.
        DiscoverProbeOutcome::DiscoverResult { supported_versions } => {
            match ctx.mutual_version(&supported_versions) {
                Some(version) => ProbeVerdict::Modern { version },
                // The peer speaks discover but advertises nothing we support.
                // This is a real incompatibility, not a legacy signal: it told
                // us its versions and none of them work.
                None => ProbeVerdict::Error(ProbeError::NoOverlap {
                    server_supported: supported_versions,
                }),
            }
        }

        // A result we cannot parse is not modern evidence, so the conservative
        // default applies.
        DiscoverProbeOutcome::UnparseableResult => legacy_or_error(
            ctx,
            ProbeError::Rpc(ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                "server/discover returned an unparseable result",
                None,
            )),
        ),

        DiscoverProbeOutcome::RpcError(error) => classify_rpc_error(error, ctx),

        // Body inspection is scoped to 400 Bad Request. Modern servers also use
        // 400 for the recognized modern errors, so a 400 must be inspected
        // before falling back.
        DiscoverProbeOutcome::HttpRejection {
            status: 400,
            jsonrpc_error,
        } => match jsonrpc_error {
            // "If the body contains a recognized modern JSON-RPC error, the
            // server speaks a modern version of MCP -- retry [...] rather than
            // falling back."
            Some(error) if is_recognized_modern_error(error.code) => classify_rpc_error(error, ctx),
            // "If the body is empty or is not a recognized modern JSON-RPC
            // error, fall back to `initialize`."
            Some(error) => legacy_or_error(ctx, ProbeError::Rpc(error)),
            None => legacy_or_error(
                ctx,
                ProbeError::Rpc(ErrorData::new(
                    ErrorCode::INTERNAL_ERROR,
                    "HTTP 400 with no JSON-RPC error body",
                    None,
                )),
            ),
        },

        // Every other status surfaces as itself. Notably 401/403 must keep their
        // `WWW-Authenticate` challenge intact for the reactive OAuth flow, and
        // 5xx is an outage rather than an era signal.
        DiscoverProbeOutcome::HttpRejection {
            status,
            jsonrpc_error,
        } => ProbeVerdict::Error(ProbeError::Rpc(jsonrpc_error.unwrap_or_else(|| {
            ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                format!("HTTP {status} during server/discover"),
                None,
            )
        }))),

        // "or does not respond within a reasonable timeout: the server is
        // legacy" -- but only where the binding says silence carries meaning.
        DiscoverProbeOutcome::NoResponse => {
            if ctx.silence_is_legacy {
                legacy_or_error(ctx, ProbeError::NoResponse)
            } else {
                ProbeVerdict::Error(ProbeError::NoResponse)
            }
        }

        // "or not at all" covers a peer that closes instead of answering.
        DiscoverProbeOutcome::Closed => {
            if ctx.silence_is_legacy {
                legacy_or_error(ctx, ProbeError::Closed)
            } else {
                ProbeVerdict::Error(ProbeError::Closed)
            }
        }
    }
}

/// Classify an in-band JSON-RPC error.
fn classify_rpc_error(error: ErrorData, ctx: &ProbeContext) -> ProbeVerdict {
    // `-32022` is the only recognized modern error that version selection can
    // act on, because it is the only one that advertises supported versions.
    if error.code == ErrorCode::UNSUPPORTED_PROTOCOL_VERSION {
        let supported = parse_supported_versions(&error);

        // Without an actionable `data.supported` list this is not usable modern
        // evidence, so the conservative default applies.
        let Some(supported) = supported else {
            return legacy_or_error(ctx, ProbeError::Rpc(error));
        };

        // Retry while the loop can still make progress.
        if let Some(version) = ctx.next_version(&supported) {
            return ProbeVerdict::RetryVersion { version };
        }

        // Out of moves. The peer is provably modern, so never fall back --
        // `-32022` is modern evidence.
        return ProbeVerdict::Error(ProbeError::NoOverlap {
            server_supported: supported,
        });
    }

    // The remaining recognized modern errors prove the peer is modern but are
    // not resolvable by version selection. Surface them; never fall back.
    if is_recognized_modern_error(error.code) {
        return ProbeVerdict::Error(ProbeError::Rpc(error));
    }

    // Everything else -- `-32601`, `-32602`, and any implementation-defined
    // code -- is legacy evidence. This is the denylist the `MUST NOT` requires.
    legacy_or_error(ctx, ProbeError::Rpc(error))
}

/// Resolve a legacy verdict, honoring whether fallback is permitted.
fn legacy_or_error(ctx: &ProbeContext, error: ProbeError) -> ProbeVerdict {
    if ctx.fallback_available {
        ProbeVerdict::Legacy
    } else {
        ProbeVerdict::Error(error)
    }
}

/// Extract `data.supported` from an `UnsupportedProtocolVersionError`.
///
/// Returns `None` when the field is absent, malformed, or empty, which makes the
/// error unusable as actionable modern evidence.
fn parse_supported_versions(error: &ErrorData) -> Option<Vec<ProtocolVersion>> {
    let supported = error.data.as_ref()?.get("supported")?.clone();
    let versions: Vec<ProtocolVersion> = serde_json::from_value(supported).ok()?;
    (!versions.is_empty()).then_some(versions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v2026() -> ProtocolVersion {
        ProtocolVersion::V_2026_07_28
    }

    fn future_version() -> ProtocolVersion {
        serde_json::from_value(serde_json::json!("2099-01-01")).unwrap()
    }

    /// Default stdio-like context: fallback permitted, silence is legacy.
    fn stdio_ctx() -> ProbeContext {
        ProbeContext {
            preferred_versions: vec![v2026()],
            attempted: vec![v2026()],
            requested_version: v2026(),
            fallback_available: true,
            silence_is_legacy: true,
        }
    }

    /// HTTP-like context: fallback permitted, silence is an outage.
    fn http_ctx() -> ProbeContext {
        ProbeContext {
            silence_is_legacy: false,
            ..stdio_ctx()
        }
    }

    fn error(code: ErrorCode) -> ErrorData {
        ErrorData::new(code, "test", None)
    }

    fn classify(outcome: DiscoverProbeOutcome, ctx: &ProbeContext) -> ProbeVerdict {
        classify_probe_outcome(outcome, ctx)
    }

    // -- Successful discovery -------------------------------------------------

    #[test]
    fn discover_result_with_overlap_is_modern() {
        let verdict = classify(
            DiscoverProbeOutcome::DiscoverResult {
                supported_versions: vec![v2026()],
            },
            &stdio_ctx(),
        );
        assert!(matches!(
            verdict,
            ProbeVerdict::Modern { version } if version == v2026()
        ));
    }

    #[test]
    fn discover_result_honors_client_preference_order() {
        let ctx = ProbeContext {
            preferred_versions: vec![future_version(), v2026()],
            ..stdio_ctx()
        };
        let verdict = classify(
            DiscoverProbeOutcome::DiscoverResult {
                supported_versions: vec![v2026(), future_version()],
            },
            &ctx,
        );
        assert!(
            matches!(verdict, ProbeVerdict::Modern { version } if version == future_version()),
            "the client's first preference must win"
        );
    }

    /// A discover-answering peer with no shared version told us its versions:
    /// that is an incompatibility, not a legacy signal.
    #[test]
    fn discover_result_without_overlap_is_not_legacy() {
        let verdict = classify(
            DiscoverProbeOutcome::DiscoverResult {
                supported_versions: vec![future_version()],
            },
            &stdio_ctx(),
        );
        assert!(matches!(
            verdict,
            ProbeVerdict::Error(ProbeError::NoOverlap { .. })
        ));
    }

    // -- The MUST NOT: fallback is not keyed to one code ----------------------

    /// The spec names `-32601` and `-32602` together as common legacy responses.
    #[test]
    fn common_legacy_error_codes_fall_back() {
        for code in [ErrorCode::METHOD_NOT_FOUND, ErrorCode::INVALID_PARAMS] {
            let verdict = classify(DiscoverProbeOutcome::RpcError(error(code)), &stdio_ctx());
            assert!(
                matches!(verdict, ProbeVerdict::Legacy),
                "{code:?} must fall back"
            );
        }
    }

    /// "implementation-defined errors" -- the denylist must cover codes we have
    /// never seen, which is the property a code-keyed allowlist cannot have.
    #[test]
    fn arbitrary_error_codes_fall_back() {
        for code in [-32000, -32603, -1, 0, 42, i32::MIN, i32::MAX] {
            let verdict = classify(
                DiscoverProbeOutcome::RpcError(error(ErrorCode(code))),
                &stdio_ctx(),
            );
            assert!(
                matches!(verdict, ProbeVerdict::Legacy),
                "unrecognized code {code} must fall back"
            );
        }
    }

    // -- Recognized modern errors never fall back ----------------------------

    #[test]
    fn missing_required_client_capability_is_modern() {
        let verdict = classify(
            DiscoverProbeOutcome::RpcError(error(ErrorCode::MISSING_REQUIRED_CLIENT_CAPABILITY)),
            &stdio_ctx(),
        );
        assert!(
            matches!(
                verdict,
                ProbeVerdict::Error(ProbeError::Rpc(data))
                    if data.code == ErrorCode::MISSING_REQUIRED_CLIENT_CAPABILITY
            ),
            "-32021 is modern evidence and must surface, not fall back"
        );
    }

    #[test]
    fn header_mismatch_is_modern() {
        let verdict = classify(
            DiscoverProbeOutcome::RpcError(error(ErrorCode::HEADER_MISMATCH)),
            &stdio_ctx(),
        );
        assert!(
            matches!(
                verdict,
                ProbeVerdict::Error(ProbeError::Rpc(data))
                    if data.code == ErrorCode::HEADER_MISMATCH
            ),
            "-32020 is a header-validation failure and must surface, not fall back"
        );
    }

    /// Even with fallback unavailable, recognized modern errors are unchanged --
    /// they were never eligible for fallback.
    #[test]
    fn recognized_modern_errors_ignore_fallback_availability() {
        for code in RECOGNIZED_MODERN_ERROR_CODES {
            if code == ErrorCode::UNSUPPORTED_PROTOCOL_VERSION {
                continue; // covered separately: it has retry semantics
            }
            for fallback_available in [true, false] {
                let ctx = ProbeContext {
                    fallback_available,
                    ..stdio_ctx()
                };
                let verdict = classify(DiscoverProbeOutcome::RpcError(error(code)), &ctx);
                assert!(
                    matches!(verdict, ProbeVerdict::Error(_)),
                    "{code:?} must never fall back (fallback_available={fallback_available})"
                );
            }
        }
    }

    // -- Version negotiation via -32022 --------------------------------------

    #[test]
    fn unsupported_version_retries_at_a_mutual_version() {
        let ctx = ProbeContext {
            preferred_versions: vec![future_version(), v2026()],
            attempted: vec![future_version()],
            ..stdio_ctx()
        };
        let verdict = classify(
            DiscoverProbeOutcome::RpcError(ErrorData::unsupported_protocol_version(
                future_version(),
                &[v2026()],
            )),
            &ctx,
        );
        assert!(matches!(
            verdict,
            ProbeVerdict::RetryVersion { version } if version == v2026()
        ));
    }

    /// A disjoint but modern `supported` list is a real incompatibility.
    #[test]
    fn unsupported_version_without_mutual_version_does_not_fall_back() {
        let verdict = classify(
            DiscoverProbeOutcome::RpcError(ErrorData::unsupported_protocol_version(
                v2026(),
                &[future_version()],
            )),
            &stdio_ctx(),
        );
        match verdict {
            ProbeVerdict::Error(ProbeError::NoOverlap { server_supported }) => {
                assert_eq!(server_supported, vec![future_version()]);
            }
            other => panic!("-32022 must never fall back, got {other:?}"),
        }
    }

    /// A server that rejects version X while naming X in its own `supported`
    /// list is answering incoherently. Released behavior re-probes X exactly
    /// once, which resolves the common case where the rejection referred to a
    /// different field. Locked in by the integration test
    /// `discover_startup_retries_current_version_once_when_server_reports_it_supported`.
    #[test]
    fn unsupported_version_retries_the_current_version_once() {
        let ctx = ProbeContext {
            preferred_versions: vec![v2026()],
            attempted: vec![v2026()],
            requested_version: v2026(),
            ..stdio_ctx()
        };
        let verdict = classify(
            DiscoverProbeOutcome::RpcError(ErrorData::unsupported_protocol_version(
                v2026(),
                &[v2026()],
            )),
            &ctx,
        );
        assert!(
            matches!(verdict, ProbeVerdict::RetryVersion { version } if version == v2026()),
            "the first incoherent rejection must re-probe the same version once"
        );
    }

    /// The other half of that rule, and the property that guarantees the retry
    /// loop terminates: a version already attempted *twice* is not tried again.
    #[test]
    fn unsupported_version_does_not_retry_a_twice_attempted_version() {
        let ctx = ProbeContext {
            preferred_versions: vec![v2026()],
            attempted: vec![v2026(), v2026()],
            requested_version: v2026(),
            ..stdio_ctx()
        };
        let verdict = classify(
            DiscoverProbeOutcome::RpcError(ErrorData::unsupported_protocol_version(
                v2026(),
                &[v2026()],
            )),
            &ctx,
        );
        assert!(
            matches!(verdict, ProbeVerdict::Error(_)),
            "a twice-attempted version must not be retried, or the loop cannot terminate"
        );
    }

    /// `-32022` without an actionable `supported` list is not usable modern
    /// evidence, so the conservative default applies.
    #[test]
    fn unsupported_version_without_supported_data_falls_back() {
        for data in [
            None,
            Some(serde_json::json!({})),
            Some(serde_json::json!({ "supported": [] })),
            Some(serde_json::json!({ "supported": "not-a-list" })),
            Some(serde_json::json!({ "supported": [17] })),
        ] {
            let verdict = classify(
                DiscoverProbeOutcome::RpcError(ErrorData::new(
                    ErrorCode::UNSUPPORTED_PROTOCOL_VERSION,
                    "unsupported",
                    data.clone(),
                )),
                &stdio_ctx(),
            );
            assert!(
                matches!(verdict, ProbeVerdict::Legacy),
                "unactionable -32022 data {data:?} must fall back"
            );
        }
    }

    // -- HTTP: inspection is scoped to 400 -----------------------------------

    #[test]
    fn http_400_with_recognized_modern_error_retries() {
        let verdict = classify(
            DiscoverProbeOutcome::HttpRejection {
                status: 400,
                jsonrpc_error: Some(ErrorData::unsupported_protocol_version(
                    future_version(),
                    &[v2026()],
                )),
            },
            &ProbeContext {
                preferred_versions: vec![future_version(), v2026()],
                attempted: vec![future_version()],
                ..http_ctx()
            },
        );
        assert!(matches!(
            verdict,
            ProbeVerdict::RetryVersion { version } if version == v2026()
        ));
    }

    #[test]
    fn http_400_with_unrecognized_error_falls_back() {
        for code in [
            ErrorCode::METHOD_NOT_FOUND,
            ErrorCode::INVALID_PARAMS,
            ErrorCode(-32000),
        ] {
            let verdict = classify(
                DiscoverProbeOutcome::HttpRejection {
                    status: 400,
                    jsonrpc_error: Some(error(code)),
                },
                &http_ctx(),
            );
            assert!(
                matches!(verdict, ProbeVerdict::Legacy),
                "HTTP 400 + {code:?} must fall back"
            );
        }
    }

    #[test]
    fn http_400_with_empty_body_falls_back() {
        let verdict = classify(
            DiscoverProbeOutcome::HttpRejection {
                status: 400,
                jsonrpc_error: None,
            },
            &http_ctx(),
        );
        assert!(matches!(verdict, ProbeVerdict::Legacy));
    }

    /// The load-bearing consequence of scoping inspection to 400: the very same
    /// JSON-RPC error that falls back on a 400 must *not* fall back on any other
    /// status.
    #[test]
    fn non_400_statuses_never_fall_back() {
        for status in [200, 401, 403, 404, 418, 500, 502, 503] {
            let verdict = classify(
                DiscoverProbeOutcome::HttpRejection {
                    status,
                    jsonrpc_error: Some(error(ErrorCode::METHOD_NOT_FOUND)),
                },
                &http_ctx(),
            );
            assert!(
                matches!(verdict, ProbeVerdict::Error(_)),
                "HTTP {status} must not be a fallback trigger"
            );
        }
    }

    #[test]
    fn http_500_with_internal_error_does_not_fall_back() {
        let verdict = classify(
            DiscoverProbeOutcome::HttpRejection {
                status: 500,
                jsonrpc_error: Some(error(ErrorCode::INTERNAL_ERROR)),
            },
            &http_ctx(),
        );
        assert!(matches!(verdict, ProbeVerdict::Error(_)));
    }

    // -- Silence: transport-dependent ----------------------------------------

    /// On stdio the spec makes a timeout legacy evidence.
    #[test]
    fn stdio_silence_is_legacy() {
        for outcome in [
            DiscoverProbeOutcome::NoResponse,
            DiscoverProbeOutcome::Closed,
        ] {
            let verdict = classify(outcome.clone(), &stdio_ctx());
            assert!(
                matches!(verdict, ProbeVerdict::Legacy),
                "{outcome:?} must be legacy evidence on stdio"
            );
        }
    }

    /// On HTTP a deployed server answers, so silence is an outage.
    #[test]
    fn http_silence_is_an_error() {
        for outcome in [
            DiscoverProbeOutcome::NoResponse,
            DiscoverProbeOutcome::Closed,
        ] {
            let verdict = classify(outcome.clone(), &http_ctx());
            assert!(
                matches!(verdict, ProbeVerdict::Error(_)),
                "{outcome:?} must not be era evidence on HTTP"
            );
        }
    }

    // -- Explicit Discover mode ----------------------------------------------

    /// With fallback unavailable, legacy evidence becomes a typed error rather
    /// than silently starting an `initialize` handshake.
    #[test]
    fn legacy_evidence_without_fallback_is_an_error() {
        let ctx = ProbeContext {
            fallback_available: false,
            ..stdio_ctx()
        };
        for outcome in [
            DiscoverProbeOutcome::RpcError(error(ErrorCode::METHOD_NOT_FOUND)),
            DiscoverProbeOutcome::NoResponse,
            DiscoverProbeOutcome::Closed,
            DiscoverProbeOutcome::UnparseableResult,
        ] {
            let verdict = classify(outcome.clone(), &ctx);
            assert!(
                matches!(verdict, ProbeVerdict::Error(_)),
                "{outcome:?} must be an error when fallback is unavailable"
            );
        }
    }

    /// Successful discovery is unaffected by fallback availability.
    #[test]
    fn modern_verdict_is_independent_of_fallback_availability() {
        for fallback_available in [true, false] {
            let ctx = ProbeContext {
                fallback_available,
                ..stdio_ctx()
            };
            let verdict = classify(
                DiscoverProbeOutcome::DiscoverResult {
                    supported_versions: vec![v2026()],
                },
                &ctx,
            );
            assert!(matches!(verdict, ProbeVerdict::Modern { .. }));
        }
    }

    // -- Denylist direction --------------------------------------------------

    /// The structural property behind the `MUST NOT`: only the enumerated codes
    /// are modern, and everything else falls back. Sweeping a wide range of codes
    /// asserts the default *direction* rather than a hand-listed set.
    ///
    /// `-32022` is excluded because its verdict depends on `data.supported` and
    /// not on the code alone: a bare `-32022` carries no actionable version list
    /// and correctly falls back (see
    /// `unsupported_version_without_supported_data_falls_back`). Its
    /// code-plus-data behavior is covered by
    /// `unsupported_version_with_actionable_data_is_never_legacy`.
    #[test]
    fn only_the_enumerated_codes_are_modern() {
        for raw in (-32700..=-31000).chain(-100..=100) {
            let code = ErrorCode(raw);
            if code == ErrorCode::UNSUPPORTED_PROTOCOL_VERSION {
                continue;
            }
            let verdict = classify(DiscoverProbeOutcome::RpcError(error(code)), &stdio_ctx());
            if is_recognized_modern_error(code) {
                assert!(
                    matches!(verdict, ProbeVerdict::Error(_)),
                    "{code:?} is recognized modern and must not fall back"
                );
            } else {
                assert!(
                    matches!(verdict, ProbeVerdict::Legacy),
                    "{code:?} is not recognized modern and must fall back"
                );
            }
        }
    }

    /// Complements the sweep: with an actionable `data.supported` list, `-32022`
    /// is modern evidence and never falls back, whatever the overlap.
    #[test]
    fn unsupported_version_with_actionable_data_is_never_legacy() {
        let ctx = ProbeContext {
            preferred_versions: vec![future_version(), v2026()],
            attempted: vec![future_version()],
            ..stdio_ctx()
        };
        for supported in [
            vec![v2026()],
            vec![future_version()],
            vec![v2026(), future_version()],
        ] {
            let verdict = classify(
                DiscoverProbeOutcome::RpcError(ErrorData::unsupported_protocol_version(
                    future_version(),
                    &supported,
                )),
                &ctx,
            );
            assert!(
                !matches!(verdict, ProbeVerdict::Legacy),
                "actionable -32022 (supported={supported:?}) must never fall back"
            );
        }
    }

    #[test]
    fn recognized_modern_set_matches_the_specification() {
        assert_eq!(
            RECOGNIZED_MODERN_ERROR_CODES,
            [
                ErrorCode(-32022), // UnsupportedProtocolVersionError
                ErrorCode(-32021), // MissingRequiredClientCapabilityError
                ErrorCode(-32020), // header-validation failures
            ]
        );
    }
}
