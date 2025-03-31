use std::marker::PhantomData;

use common::calculator::Calculator;
use futures::{Sink, Stream};
use rmcp::{
    RoleClient, RoleServer, ServiceExt,
    service::{RunningService, RxJsonRpcMessage, ServiceRole, TxJsonRpcMessage},
};
use tokio_tungstenite::tungstenite;
use tracing_subscriber::EnvFilter;
mod common;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .init();
    start_server().await?;
    let client = http_client("ws://127.0.0.1:8001").await?;
    let tools = client.list_all_tools().await?;
    client.cancel().await?;
    tracing::info!("{:#?}", tools);
    Ok(())
}

async fn http_client(uri: &str) -> anyhow::Result<RunningService<RoleClient, ()>> {
    let (stream, response) = tokio_tungstenite::connect_async(uri).await?;
    if response.status() != tungstenite::http::StatusCode::SWITCHING_PROTOCOLS {
        return Err(anyhow::anyhow!("failed to upgrade connection"));
    }
    let transport = WebsocketTransport::new_client(stream);
    let client = ().serve(transport).await?;
    Ok(client)
}

async fn start_server() -> anyhow::Result<()> {
    let tcp_listener = tokio::net::TcpListener::bind("127.0.0.1:8001").await?;
    tokio::spawn(async move {
        while let Ok((stream, addr)) = tcp_listener.accept().await {
            tracing::info!("accepted connection from: {}", addr);
            tokio::spawn(async move {
                let ws_stream = tokio_tungstenite::accept_async(stream).await?;
                let transport = WebsocketTransport::new_server(ws_stream);
                let server = Calculator.serve(transport).await?;
                server.waiting().await?;
                Ok::<(), anyhow::Error>(())
            });
        }
    });
    Ok(())
}

pin_project_lite::pin_project! {
    pub struct WebsocketTransport<R, S, E> {
        #[pin]
        stream: S,
        marker: PhantomData<(fn() -> E, fn() -> R)>
    }
}

impl<R, S, E> WebsocketTransport<R, S, E> {
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            marker: PhantomData,
        }
    }
}

impl<S, E> WebsocketTransport<RoleClient, S, E> {
    pub fn new_client(stream: S) -> Self {
        Self {
            stream,
            marker: PhantomData,
        }
    }
}

impl<S, E> WebsocketTransport<RoleServer, S, E> {
    pub fn new_server(stream: S) -> Self {
        Self {
            stream,
            marker: PhantomData,
        }
    }
}

impl<R, S, E> Stream for WebsocketTransport<R, S, E>
where
    S: Stream<Item = Result<tungstenite::Message, E>>,
    R: ServiceRole,
    E: std::error::Error,
{
    type Item = RxJsonRpcMessage<R>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.as_mut().project();
        match this.stream.poll_next(cx) {
            std::task::Poll::Ready(Some(Ok(message))) => {
                let message = match message {
                    tungstenite::Message::Text(json) => json,
                    _ => return self.poll_next(cx),
                };
                let message = match serde_json::from_str::<RxJsonRpcMessage<R>>(&message) {
                    Ok(message) => message,
                    Err(e) => {
                        tracing::warn!(error = %e, "serde_json parse error");
                        return self.poll_next(cx);
                    }
                };
                std::task::Poll::Ready(Some(message))
            }
            std::task::Poll::Ready(Some(Err(e))) => {
                tracing::warn!(error = %e, "websocket error");
                self.poll_next(cx)
            }
            std::task::Poll::Ready(None) => std::task::Poll::Ready(None),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

impl<R, S, E> Sink<TxJsonRpcMessage<R>> for WebsocketTransport<R, S, E>
where
    S: Sink<tungstenite::Message, Error = E>,
    R: ServiceRole,
{
    type Error = E;

    fn poll_ready(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        let this = self.project();
        this.stream.poll_ready(cx)
    }

    fn start_send(
        self: std::pin::Pin<&mut Self>,
        item: TxJsonRpcMessage<R>,
    ) -> Result<(), Self::Error> {
        let this = self.project();
        let message = tungstenite::Message::Text(
            serde_json::to_string(&item)
                .expect("jsonrpc should be valid json")
                .into(),
        );
        this.stream.start_send(message)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        let this = self.project();
        this.stream.poll_flush(cx)
    }

    fn poll_close(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        let this = self.project();
        this.stream.poll_close(cx)
    }
}
