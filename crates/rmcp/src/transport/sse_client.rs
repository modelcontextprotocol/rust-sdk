//！ reference: https://html.spec.whatwg.org/multipage/server-sent-events.html
use std::{pin::Pin, sync::Arc};

use futures::{StreamExt, future::BoxFuture};
use reqwest::header::HeaderValue;
use sse_stream::Error as SseError;
use thiserror::Error;

use super::{
    Transport,
    common::client_side_sse::{BoxedSseResponse, SseRetryPolicy, SseStreamReconnect},
};
use crate::{
    RoleClient,
    model::{ClientJsonRpcMessage, ServerJsonRpcMessage},
    transport::common::client_side_sse::SseAutoReconnectStream,
};

#[derive(Error, Debug)]
pub enum SseTransportError<E: std::error::Error + Send + Sync + 'static> {
    #[error("SSE error: {0}")]
    Sse(#[from] SseError),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Client error: {0}")]
    Client(E),
    #[error("unexpected end of stream")]
    UnexpectedEndOfStream,
    #[error("Url error: {0}")]
    Url(#[from] url::ParseError),
    #[error("Unexpected content type: {0:?}")]
    UnexpectedContentType(Option<HeaderValue>),
    #[error("Tokio join error: {0}")]
    TokioJoinError(#[from] tokio::task::JoinError),
    #[error("Transport terminated")]
    TransportTerminated,
    #[cfg(feature = "__auth")]
    #[cfg_attr(docsrs, doc(cfg(feature = "__auth")))]
    #[error("Auth error: {0}")]
    Auth(#[from] crate::transport::auth::AuthError),
}

impl From<reqwest::Error> for SseTransportError<reqwest::Error> {
    fn from(e: reqwest::Error) -> Self {
        SseTransportError::Client(e)
    }
}

pub trait SseClient: Clone + Send + Sync + 'static {
    type Error: std::error::Error + Send + Sync + 'static;
    fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        auth_token: Option<String>,
    ) -> impl Future<Output = Result<(), SseTransportError<Self::Error>>> + Send + '_;
    fn get_stream(
        &self,
        uri: Arc<str>,
        last_event_id: Option<String>,
        auth_token: Option<String>,
    ) -> impl Future<Output = Result<BoxedSseResponse, SseTransportError<Self::Error>>> + Send + '_;
}

struct SseClientReconnect<C> {
    pub client: C,
    pub uri: Arc<str>,
}

impl<C: SseClient> SseStreamReconnect for SseClientReconnect<C> {
    type Error = SseTransportError<C::Error>;
    type Future = BoxFuture<'static, Result<BoxedSseResponse, Self::Error>>;
    fn retry_connection(&mut self, last_event_id: Option<&str>) -> Self::Future {
        let client = self.client.clone();
        let uri = self.uri.clone();
        let last_event_id = last_event_id.map(|s| s.to_owned());
        Box::pin(async move { client.get_stream(uri, last_event_id, None).await })
    }
}
type ServerMessageStream<C> = Pin<Box<SseAutoReconnectStream<SseClientReconnect<C>>>>;
pub struct SseClientTransport<C: SseClient> {
    client: C,
    config: SseClientConfig,
    post_uri: Arc<str>,
    stream: Option<ServerMessageStream<C>>,
}

impl<C: SseClient> Transport<RoleClient> for SseClientTransport<C> {
    type Error = SseTransportError<C::Error>;
    async fn receive(&mut self) -> Option<ServerJsonRpcMessage> {
        self.stream.as_mut()?.next().await?.ok()
    }
    fn send(
        &mut self,
        item: crate::service::TxJsonRpcMessage<RoleClient>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let client = self.client.clone();
        let uri = self.post_uri.clone();
        async move { client.post_message(uri, item, None).await }
    }
    async fn close(&mut self) -> Result<(), Self::Error> {
        self.stream.take();
        Ok(())
    }
}

impl<C: SseClient + std::fmt::Debug> std::fmt::Debug for SseClientTransport<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SseClientWorker")
            .field("client", &self.client)
            .field("config", &self.config)
            .finish()
    }
}

impl<C: SseClient> SseClientTransport<C> {
    pub async fn start_with_client(
        client: C,
        config: SseClientConfig,
    ) -> Result<Self, SseTransportError<C::Error>> {
        let mut sse_stream = client.get_stream(config.uri.clone(), None, None).await?;
        // wait the endpoint event
        let endpoint = loop {
            let sse = sse_stream
                .next()
                .await
                .ok_or(SseTransportError::UnexpectedEndOfStream)??;
            let Some("endpoint") = sse.event.as_deref() else {
                continue;
            };
            break sse.data.unwrap_or_default();
        };
        let post_uri: Arc<str> = format!(
            "{}/{}",
            config.uri.trim_end_matches("/"),
            endpoint.trim_start_matches("/")
        )
        .into();
        let stream = Box::pin(SseAutoReconnectStream::new(
            sse_stream,
            SseClientReconnect {
                client: client.clone(),
                uri: config.uri.clone(),
            },
            config.retry_policy.clone(),
        ));
        Ok(Self {
            client,
            config,
            post_uri,
            stream: Some(stream),
        })
    }
}

#[derive(Debug, Clone)]
pub struct SseClientConfig {
    pub uri: Arc<str>,
    pub retry_policy: Arc<dyn SseRetryPolicy>,
}

impl Default for SseClientConfig {
    fn default() -> Self {
        Self {
            uri: "".into(),
            retry_policy: Arc::new(super::common::client_side_sse::FixedInterval::default()),
        }
    }
}
