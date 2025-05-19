//！ reference: https://html.spec.whatwg.org/multipage/server-sent-events.html
use std::{pin::Pin, sync::Arc};

use futures::{StreamExt, future::BoxFuture};
use http::Uri;
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
    #[error("Unexpected content type: {0:?}")]
    UnexpectedContentType(Option<HeaderValue>),
    #[cfg(feature = "auth")]
    #[cfg_attr(docsrs, doc(cfg(feature = "auth")))]
    #[error("Auth error: {0}")]
    Auth(#[from] crate::transport::auth::AuthError),
    #[error("Invalid uri: {0}")]
    InvalidUri(#[from] http::uri::InvalidUri),
    #[error("Invalid uri parts: {0}")]
    InvalidUriParts(#[from] http::uri::InvalidUriParts),
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
        uri: Uri,
        message: ClientJsonRpcMessage,
        auth_token: Option<String>,
    ) -> impl Future<Output = Result<(), SseTransportError<Self::Error>>> + Send + '_;
    fn get_stream(
        &self,
        uri: Uri,
        last_event_id: Option<String>,
        auth_token: Option<String>,
    ) -> impl Future<Output = Result<BoxedSseResponse, SseTransportError<Self::Error>>> + Send + '_;
}

struct SseClientReconnect<C> {
    pub client: C,
    pub uri: Uri,
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
    message_endpoint: Uri,
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
        let uri = self.message_endpoint.clone();
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
        let sse_endpoint = config.sse_endpoint.as_ref().parse::<http::Uri>()?;

        let mut sse_stream = client.get_stream(sse_endpoint.clone(), None, None).await?;
        let message_endpoint = if let Some(endpoint) = config.use_message_endpoint.clone() {
            endpoint.parse::<http::Uri>()?
        } else {
            // wait the endpoint event
            loop {
                let sse = sse_stream
                    .next()
                    .await
                    .ok_or(SseTransportError::UnexpectedEndOfStream)??;
                let Some("endpoint") = sse.event.as_deref() else {
                    continue;
                };
                let sse_endpoint = sse.data.unwrap_or_default();
                break sse_endpoint.parse::<http::Uri>()?;
            }
        };

        // sse: <authority><sse_pq> -> <authority><message_pq>
        let message_endpoint = {
            let mut sse_endpoint_parts = sse_endpoint.clone().into_parts();
            sse_endpoint_parts.path_and_query = message_endpoint.into_parts().path_and_query;
            Uri::from_parts(sse_endpoint_parts)?
        };
        let stream = Box::pin(SseAutoReconnectStream::new(
            sse_stream,
            SseClientReconnect {
                client: client.clone(),
                uri: sse_endpoint.clone(),
            },
            config.retry_policy.clone(),
        ));
        Ok(Self {
            client,
            config,
            message_endpoint,
            stream: Some(stream),
        })
    }
}

#[derive(Debug, Clone)]
pub struct SseClientConfig {
    /// client sse endpoint
    ///
    /// # How this client resolve the message endpoint
    /// if sse_endpoint has this format: `<schema><authority?><sse_pq>`,
    /// then the message endpoint will be `<schema><authority?><message_pq>`.
    ///
    /// For example, if you config the sse_endpoint as `http://example.com/some_path/sse`,
    /// and the server send the message endpoint event as `message?session_id=123`,
    /// then the message endpoint will be `http://example.com/message`.
    ///
    /// This follow the rules of JavaScript's [`new URL(url, base)`](https://developer.mozilla.org/zh-CN/docs/Web/API/URL/URL)
    pub sse_endpoint: Arc<str>,
    pub retry_policy: Arc<dyn SseRetryPolicy>,
    /// if this is settled, the client will use this endpoint to send message and skip get the endpoint event
    pub use_message_endpoint: Option<String>,
}

impl Default for SseClientConfig {
    fn default() -> Self {
        Self {
            sse_endpoint: "".into(),
            retry_policy: Arc::new(super::common::client_side_sse::FixedInterval::default()),
            use_message_endpoint: None,
        }
    }
}
