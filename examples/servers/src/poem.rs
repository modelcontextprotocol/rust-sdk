use futures::StreamExt;
use mcp_server::{ByteTransport, Server as McpServer};
use poem::{
    handler,
    http::StatusCode,
    listener::TcpListener,
    web::{
        sse::{Event, SSE},
        Data, Query,
    },
    Body, EndpointExt, Error, IntoResponse, Route, Server,
};
use std::collections::HashMap;
use tokio_util::codec::FramedRead;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use mcp_server::router::RouterService;
use std::sync::Arc;
use tokio::{
    io::{self, AsyncWriteExt},
    sync::Mutex,
};
use tracing_subscriber::{self};

mod common;
use common::counter;

type C2SWriter = Arc<Mutex<io::WriteHalf<io::SimplexStream>>>;
type SessionId = Arc<str>;

const BIND_ADDRESS: &str = "127.0.0.1:8000";

#[derive(Clone, Default)]
pub struct App {
    txs: Arc<tokio::sync::RwLock<HashMap<SessionId, C2SWriter>>>,
}

impl App {
    pub fn new() -> Self {
        Self {
            txs: Default::default(),
        }
    }

    pub fn route(&self) -> impl poem::Endpoint {
        Route::new()
            .at("/sse", poem::get(sse_handler).post(post_event_handler))
            .data(self.clone())
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

#[handler]
async fn post_event_handler(
    app: Data<&App>,
    Query(query): Query<PostEventQuery>,
    body: Body,
) -> poem::Result<impl IntoResponse> {
    const BODY_BYTES_LIMIT: usize = 1 << 22;
    let write_stream = {
        let rg = app.txs.read().await;
        rg.get(query.session_id.as_str())
            .ok_or_else(|| Error::from_string("Session not found", StatusCode::NOT_FOUND))?
            .clone()
    };
    let mut write_stream = write_stream.lock().await;
    let bytes = body.into_bytes().await?;
    if bytes.len() > BODY_BYTES_LIMIT {
        return Err(Error::from_string(
            "Payload too large",
            StatusCode::PAYLOAD_TOO_LARGE,
        ));
    }
    write_stream
        .write_all(&bytes)
        .await
        .map_err(|e| Error::from_string(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    write_stream
        .write_u8(b'\n')
        .await
        .map_err(|e| Error::from_string(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    Ok(StatusCode::ACCEPTED)
}

#[handler]
async fn sse_handler(app: Data<&App>) -> impl IntoResponse {
    const BUFFER_SIZE: usize = 1 << 12;
    let session = session_id();
    tracing::info!(%session, "sse connection");
    let (c2s_read, c2s_write) = tokio::io::simplex(BUFFER_SIZE);
    let (s2c_read, s2c_write) = tokio::io::simplex(BUFFER_SIZE);
    app.txs
        .write()
        .await
        .insert(session.clone(), Arc::new(Mutex::new(c2s_write)));

    {
        let session = session.clone();
        let app = app.clone();
        tokio::spawn(async move {
            let router = RouterService(counter::CounterRouter::new());
            let server = McpServer::new(router);
            let bytes_transport = ByteTransport::new(c2s_read, s2c_write);
            let _result = server
                .run(bytes_transport)
                .await
                .inspect_err(|e| tracing::error!(?e, "server run error"));
            app.txs.write().await.remove(&session);
        });
    }

    let stream = futures::stream::once(futures::future::ready(
        Event::message(format!("?sessionId={session}")).event_type("endpoint"),
    ))
    .chain(
        FramedRead::new(s2c_read, common::jsonrpc_frame_codec::JsonRpcFrameCodec).map(|result| {
            match result {
                Ok(bytes) => match std::str::from_utf8(&bytes) {
                    Ok(message) => Event::message(message),
                    Err(e) => Event::message(format!("Error: {}", e)),
                },
                Err(e) => Event::message(format!("Error: {}", e)),
            }
        }),
    );

    SSE::new(stream)
}

#[tokio::main]
async fn main() -> io::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| format!("info,{}=debug", env!("CARGO_CRATE_NAME")).into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let app = App::new();
    let listener = TcpListener::bind(BIND_ADDRESS);

    tracing::debug!("listening on {}", BIND_ADDRESS);
    Server::new(listener).run(app.route()).await?;
    Ok(())
}
