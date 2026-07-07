#![cfg(not(feature = "local"))]
//! SEP-414: the reserved trace-context `_meta` keys survive a client→server round trip unchanged.
use std::sync::Arc;

use rmcp::{
    RoleServer, ServerHandler, ServiceExt,
    model::{ClientRequest, CustomRequest, CustomResult, Meta},
    service::{PeerRequestOptions, RequestContext},
};
use serde_json::json;
use tokio::sync::{Mutex, Notify};

const TRACEPARENT: &str = "00-0af7651916cd43dd8448eb211c80319c-00f067aa0ba902b7-01";
const TRACESTATE: &str = "vendor1=value1,vendor2=value2";
const BAGGAGE: &str = "userId=alice,region=us-east-1";

/// Records the `_meta` it receives on the incoming request so the test can assert passthrough.
struct TraceCapturingServer {
    receive_signal: Arc<Notify>,
    seen: Arc<Mutex<Option<Meta>>>,
}

impl ServerHandler for TraceCapturingServer {
    async fn on_custom_request(
        &self,
        _request: CustomRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<CustomResult, rmcp::ErrorData> {
        *self.seen.lock().await = Some(context.meta);
        self.receive_signal.notify_one();
        Ok(CustomResult::new(json!({ "status": "ok" })))
    }
}

#[tokio::test]
async fn trace_context_meta_survives_round_trip() -> anyhow::Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let receive_signal = Arc::new(Notify::new());
    let seen = Arc::new(Mutex::new(None));

    {
        let receive_signal = receive_signal.clone();
        let seen = seen.clone();
        tokio::spawn(async move {
            let server = TraceCapturingServer {
                receive_signal,
                seen,
            }
            .serve(server_transport)
            .await?;
            server.waiting().await?;
            anyhow::Ok(())
        });
    }

    let client = ().serve(client_transport).await?;

    // Client attaches trace context to the outgoing request's `_meta`.
    let mut meta = Meta::new();
    meta.set_traceparent(TRACEPARENT);
    meta.set_tracestate(TRACESTATE);
    meta.set_baggage(BAGGAGE);

    let mut options = PeerRequestOptions::no_options();
    options.meta = Some(meta);
    client
        .send_cancellable_request(
            ClientRequest::CustomRequest(CustomRequest::new("requests/trace-test", None)),
            options,
        )
        .await?
        .await_response()
        .await?;

    tokio::time::timeout(std::time::Duration::from_secs(5), receive_signal.notified()).await?;

    // Server saw the reserved keys unchanged (alongside the injected progressToken).
    let seen = seen.lock().await.take().expect("server observed meta");
    assert_eq!(seen.get_traceparent(), Some(TRACEPARENT));
    assert_eq!(seen.get_tracestate(), Some(TRACESTATE));
    assert_eq!(seen.get_baggage(), Some(BAGGAGE));

    client.cancel().await?;
    Ok(())
}
