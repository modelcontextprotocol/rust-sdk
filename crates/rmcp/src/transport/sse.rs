//！ reference: https://html.spec.whatwg.org/multipage/server-sent-events.html
use std::{collections::VecDeque, sync::Arc, time::Duration};

use futures::{FutureExt, Sink, Stream, StreamExt, future::BoxFuture, stream::BoxStream};
use reqwest::{
    Client as HttpClient, IntoUrl, Url,
    header::{ACCEPT, HeaderValue},
};
use sse_stream::{Error as SseError, Sse, SseStream};
use thiserror::Error;

use crate::model::{ClientJsonRpcMessage, ServerJsonRpcMessage};
const MIME_TYPE: &str = "text/event-stream";
const HEADER_LAST_EVENT_ID: &str = "Last-Event-ID";

#[derive(Error, Debug)]
pub enum SseTransportError {
    #[error("SSE error: {0}")]
    Sse(#[from] SseError),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Reqwest error: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("unexpected end of stream")]
    UnexpectedEndOfStream,
    #[error("Url error: {0}")]
    Url(#[from] url::ParseError),
    #[error("Unexpected content type: {0:?}")]
    UnexpectedContentType(Option<HeaderValue>),
}

enum SseTransportState {
    Connected(BoxStream<'static, Result<Sse, SseError>>),
    Retrying {
        times: usize,
        fut: BoxFuture<
            'static,
            Result<BoxStream<'static, Result<Sse, SseError>>, SseTransportError>,
        >,
    },
    Fatal {
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct SseTransportRetryConfig {
    pub max_times: Option<usize>,
    pub min_duration: Duration,
}
impl SseTransportRetryConfig {
    pub const DEFAULT_MIN_DURATION: Duration = Duration::from_millis(1000);
}
impl Default for SseTransportRetryConfig {
    fn default() -> Self {
        Self {
            max_times: None,
            min_duration: Self::DEFAULT_MIN_DURATION,
        }
    }
}

/// # Transport for client sse
///
/// Call [`SseTransport::start`] to create a new transport from url.
///
/// Call [`SseTransport::start_with_client`] to create a new transport with a customized reqwest client.
pub struct SseTransport {
    http_client: HttpClient,
    state: SseTransportState,
    post_url: Arc<Url>,
    sse_url: Arc<Url>,
    last_event_id: Option<String>,
    recommended_retry_duration_ms: Option<u64>,
    #[allow(clippy::type_complexity)]
    request_queue: VecDeque<tokio::sync::oneshot::Receiver<Result<(), SseTransportError>>>,
    pub retry_config: SseTransportRetryConfig,
}

impl SseTransport {
    pub async fn start_with_timeout<U>(url: U, timeout: Duration) -> Result<Self, SseTransportError>
    where
        U: IntoUrl,
    {
        let mut client = HttpClient::builder();
        client = client.timeout(timeout);
        let client = client.build()?;
        Self::start_with_client(url, client).await
    }

    pub async fn start<U>(url: U) -> Result<Self, SseTransportError>
    where
        U: IntoUrl,
    {
        Self::start_with_client(url, HttpClient::default()).await
    }

    /// Start with a reqwest client, this would be helpful when you want to customize the client behavior like default headers or tls stuff.
    pub async fn start_with_client<U>(url: U, client: HttpClient) -> Result<Self, SseTransportError>
    where
        U: IntoUrl,
    {
        let url = url.into_url()?;
        let response = client
            .get(url.clone())
            .header(ACCEPT, MIME_TYPE)
            .send()
            .await?;
        let response = response.error_for_status()?;
        match response.headers().get(reqwest::header::CONTENT_TYPE) {
            Some(ct) => {
                if !ct.as_bytes().starts_with(MIME_TYPE.as_bytes()) {
                    return Err(SseTransportError::UnexpectedContentType(Some(ct.clone())));
                }
            }
            None => {
                return Err(SseTransportError::UnexpectedContentType(None));
            }
        }
        let mut event_stream = SseStream::from_byte_stream(response.bytes_stream()).boxed();
        let mut last_event_id = None;
        let mut retry = None;
        let post_url = loop {
            let next_event = event_stream
                .next()
                .await
                .ok_or(SseTransportError::UnexpectedEndOfStream)??;
            if let Some(id) = next_event.id {
                last_event_id = Some(id);
            }
            if let Some(retry_ms) = next_event.retry {
                retry = Some(retry_ms);
            }
            if let Some("endpoint") = next_event.event.as_deref() {
                break next_event.data.unwrap_or_default();
            }
        };
        tracing::info!("will post event on {post_url}");
        let post_url = url.join(&post_url)?;
        Ok(SseTransport {
            http_client: client,
            state: SseTransportState::Connected(Box::pin(event_stream)),
            post_url: Arc::from(post_url),
            last_event_id,
            recommended_retry_duration_ms: retry,
            sse_url: Arc::from(url),
            request_queue: Default::default(),
            retry_config: Default::default(),
        })
    }

    fn retry_connection(
        &self,
    ) -> BoxFuture<'static, Result<BoxStream<'static, Result<Sse, SseError>>, SseTransportError>>
    {
        let retry_duration = {
            let recommended_retry_duration = self
                .recommended_retry_duration_ms
                .map(Duration::from_millis);
            let config_retry_duration = self.retry_config.min_duration;
            recommended_retry_duration
                .map(|d| d.max(config_retry_duration))
                .unwrap_or(config_retry_duration)
        };
        let client = self.http_client.clone();
        let sse_url = self.sse_url.as_ref().clone();
        let last_event_id = self.last_event_id.clone();
        let fut = async move {
            tokio::time::sleep(retry_duration).await;
            let mut request_builder = client.get(sse_url).header(ACCEPT, MIME_TYPE);
            if let Some(last_event_id) = last_event_id {
                request_builder = request_builder.header(HEADER_LAST_EVENT_ID, last_event_id);
            }
            let response = request_builder.send().await?;
            let response = response.error_for_status()?;
            match response.headers().get(reqwest::header::CONTENT_TYPE) {
                Some(ct) => {
                    if ct.as_bytes() != MIME_TYPE.as_bytes() {
                        return Err(SseTransportError::UnexpectedContentType(Some(ct.clone())));
                    }
                }
                None => {
                    return Err(SseTransportError::UnexpectedContentType(None));
                }
            }
            let event_stream = SseStream::from_byte_stream(response.bytes_stream()).boxed();
            Ok(event_stream)
        };
        fut.boxed()
    }
}

impl Stream for SseTransport {
    type Item = ServerJsonRpcMessage;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let retry_config = self.retry_config;
        let state = &mut self.state;
        match state {
            SseTransportState::Connected(event_stream) => {
                let event = std::task::ready!(event_stream.poll_next_unpin(cx));
                match event {
                    Some(Ok(event)) => {
                        if let Some(retry) = event.retry {
                            self.recommended_retry_duration_ms = Some(retry);
                        }
                        if let Some(id) = event.id {
                            self.last_event_id = Some(id);
                        }
                        if let Some(data) = event.data {
                            match serde_json::from_str(&data) {
                                Ok(message) => std::task::Poll::Ready(Some(message)),
                                Err(e) => {
                                    tracing::error!(error = %e, "failed to parse json rpc request");
                                    self.poll_next(cx)
                                }
                            }
                        } else {
                            self.poll_next(cx)
                        }
                    }

                    Some(Err(e)) => {
                        tracing::error!(error = %e, "sse event stream encounter an error");
                        let fut = self.retry_connection();
                        self.state = SseTransportState::Retrying { times: 1, fut };
                        self.poll_next(cx)
                    }
                    None => std::task::Poll::Ready(None),
                }
            }
            SseTransportState::Retrying { fut, times } => {
                let retry_result = std::task::ready!(fut.poll_unpin(cx));
                match retry_result {
                    Ok(stream) => {
                        self.state = SseTransportState::Connected(stream);
                        self.poll_next(cx)
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "retrying failed");
                        if let Some(max_retry_times) = retry_config.max_times {
                            if *times >= max_retry_times {
                                self.state = SseTransportState::Fatal {
                                    reason: format!("retrying failed after {} times: {}", times, e),
                                };
                                return self.poll_next(cx);
                            }
                        }
                        let times = *times + 1;
                        let fut = self.retry_connection();
                        self.state = SseTransportState::Retrying { times, fut };
                        self.poll_next(cx)
                    }
                }
            }
            SseTransportState::Fatal { reason } => {
                tracing::error!("sse transport fatal error: {}", reason);
                std::task::Poll::Ready(None)
            }
        }
    }
}

impl Sink<ClientJsonRpcMessage> for SseTransport {
    type Error = SseTransportError;

    fn poll_ready(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        const QUEUE_SIZE: usize = 16;
        if self.request_queue.len() >= QUEUE_SIZE {
            std::task::ready!(
                self.request_queue
                    .front_mut()
                    .expect("queue is not empty")
                    .poll_unpin(cx)
            )
            .expect("sender shall not drop")?;
        }
        std::task::Poll::Ready(Ok(()))
    }

    fn start_send(
        mut self: std::pin::Pin<&mut Self>,
        item: ClientJsonRpcMessage,
    ) -> Result<(), Self::Error> {
        let client = self.http_client.clone();
        let uri = self.post_url.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let request_builder = client.post(uri.as_ref().clone()).json(&item);

        tokio::spawn(async move {
            let result = request_builder
                .send()
                .await
                .and_then(|resp| resp.error_for_status())
                .map_err(SseTransportError::from)
                .map(drop);
            let _ = tx.send(result);
        });
        self.as_mut().request_queue.push_back(rx);
        Ok(())
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        let queue = &mut self.as_mut().request_queue;
        while let Some(fut) = queue.front_mut() {
            std::task::ready!(fut.poll_unpin(cx)).expect("sender shall not drop")?;
            queue.pop_front();
        }
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_close(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.poll_flush(cx)
    }
}
