use actix_web::web::{Bytes, Data, Payload, Query};
use actix_web::{get, post, App, Error, HttpResponse, HttpServer, Result};
use futures::{StreamExt, TryStreamExt};
use mcp_server::{ByteTransport, Server};
use std::collections::HashMap;
use tokio_util::codec::FramedRead;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use actix_web::middleware::Logger;
use mcp_server::router::RouterService;
use std::sync::Arc;
use tokio::{
    io::{self, AsyncWriteExt},
    sync::Mutex,
};
mod common;
use common::counter;

type C2SWriter = Arc<Mutex<io::WriteHalf<io::SimplexStream>>>;
type SessionId = Arc<str>;

const BIND_ADDRESS: &str = "127.0.0.1:8000";

#[derive(Clone, Default)]
pub struct AppState {
    txs: Arc<tokio::sync::RwLock<HashMap<SessionId, C2SWriter>>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            txs: Default::default(),
        }
    }
}

fn session_id() -> SessionId {
    let id = format!("{:016x}", rand::random::<u128>());
    Arc::from(id)
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostEventQuery {
    pub session_id: String,
}

#[post("/sse")]
async fn post_event_handler(
    app_state: Data<AppState>,
    query: Query<PostEventQuery>,
    mut payload: Payload,
) -> Result<HttpResponse, actix_web::Error> {
    const BODY_BYTES_LIMIT: usize = 1 << 22;
    let session_id = &query.session_id;

    let write_stream = {
        let rg = app_state.txs.read().await;
        match rg.get(session_id.as_str()) {
            Some(stream) => stream.clone(),
            None => return Ok(HttpResponse::NotFound().finish()),
        }
    };

    let mut write_stream = write_stream.lock().await;
    let mut size = 0;

    // Process the request body in chunks
    while let Some(chunk) = payload.next().await {
        let chunk = chunk?;
        size += chunk.len();
        if size > BODY_BYTES_LIMIT {
            return Ok(HttpResponse::PayloadTooLarge().finish());
        }

        if (write_stream.write_all(&chunk).await).is_err() {
            return Ok(HttpResponse::InternalServerError().finish());
        }
    }

    if (write_stream.write_u8(b'\n').await).is_err() {
        return Ok(HttpResponse::InternalServerError().finish());
    }

    Ok(HttpResponse::Accepted().finish())
}

#[get("/sse")]
async fn sse_handler(app_state: Data<AppState>) -> Result<HttpResponse, Error> {
    // it's 4KB
    const BUFFER_SIZE: usize = 1 << 12;
    let session = session_id();
    tracing::info!(%session, "sse connection");

    let (c2s_read, c2s_write) = tokio::io::simplex(BUFFER_SIZE);
    let (s2c_read, s2c_write) = tokio::io::simplex(BUFFER_SIZE);

    app_state
        .txs
        .write()
        .await
        .insert(session.clone(), Arc::new(Mutex::new(c2s_write)));

    {
        let session = session.clone();
        let app_state = app_state.clone();
        tokio::spawn(async move {
            let router = RouterService(counter::CounterRouter::new());
            let server = Server::new(router);
            let bytes_transport = ByteTransport::new(c2s_read, s2c_write);
            let _result = server
                .run(bytes_transport)
                .await
                .inspect_err(|e| tracing::error!(?e, "server run error"));
            tracing::info!(%session, "connection closed, removing session");
            app_state.txs.write().await.remove(&session);
        });
    }

    // Create SSE stream with correct types
    let stream = futures::stream::once(futures::future::ready(Ok::<_, io::Error>(Bytes::from(
        format!("event: endpoint\ndata: ?sessionId={}\n\n", session),
    ))))
    .chain(
        FramedRead::new(s2c_read, common::jsonrpc_frame_codec::JsonRpcFrameCodec)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
            .map_ok(move |bytes| {
                let message = match std::str::from_utf8(&bytes) {
                    Ok(message) => format!("event: message\ndata: {}\n\n", message),
                    Err(_) => "event: error\ndata: Invalid UTF-8 data\n\n".to_string(),
                };
                Bytes::from(message)
            }),
    );

    // Return SSE response
    Ok(HttpResponse::Ok()
        .append_header(("Content-Type", "text/event-stream"))
        .append_header(("Cache-Control", "no-cache"))
        .append_header(("Connection", "keep-alive"))
        .streaming(stream))
}

#[actix_web::main]
async fn main() -> io::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| format!("info,{}=debug", env!("CARGO_CRATE_NAME")).into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::debug!("starting server at {}", BIND_ADDRESS);

    let app_state = Data::new(AppState::new());

    HttpServer::new(move || {
        App::new()
            .wrap(Logger::default())
            .app_data(app_state.clone())
            .service(sse_handler)
            .service(post_event_handler)
    })
    .bind(BIND_ADDRESS)?
    .run()
    .await
}
