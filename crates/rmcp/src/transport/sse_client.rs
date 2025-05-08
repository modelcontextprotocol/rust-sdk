//！ reference: https://html.spec.whatwg.org/multipage/server-sent-events.html
use std::{sync::Arc, time::Duration};

use futures::StreamExt;
use reqwest::header::HeaderValue;
use sse_stream::{Error as SseError, Sse};
use thiserror::Error;

use super::{
    WorkerTransport,
    common::sse::{BoxedSseResponse, SseRetryConfig},
    worker::Worker,
};
use crate::{
    RoleClient,
    model::{ClientJsonRpcMessage, ServerJsonRpcMessage},
    transport::worker::{WorkerQuitReason, WorkerSendRequest},
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
#[derive(Debug, Default, Clone)]
pub struct SseClientWorker<C: SseClient> {
    pub client: C,
    pub config: SseClientConfig,
}

impl<C: SseClient> SseClientWorker<C> {
    pub fn new(client: C, config: SseClientConfig) -> Self {
        Self { client, config }
    }
}

#[derive(Debug, Default, Clone)]
pub struct SseClientConfig {
    pub uri: Arc<str>,
    pub retry_config: SseRetryConfig,
}

impl<C: SseClient> Worker for SseClientWorker<C> {
    type Error = SseTransportError<C::Error>;
    type Role = RoleClient;
    fn err_closed() -> Self::Error {
        SseTransportError::TransportTerminated
    }
    fn err_join(e: tokio::task::JoinError) -> Self::Error {
        SseTransportError::TokioJoinError(e)
    }
    async fn run(
        self,
        mut context: super::worker::WorkerContext<Self>,
    ) -> Result<(), WorkerQuitReason> {
        // get stream
        let mut sse_stream = self
            .client
            .get_stream(self.config.uri.clone(), None, None)
            .await
            .map_err(WorkerQuitReason::fatal_context("get sse stream"))?;
        // wait the endpoint event
        let endpoint = loop {
            let sse = sse_stream
                .next()
                .await
                .ok_or_else(|| {
                    WorkerQuitReason::fatal("unexpected end of stream", "get the endpoint event")
                })?
                .map_err(WorkerQuitReason::fatal_context("get the endpoint event"))?;
            let Some("endpoint") = sse.event.as_deref() else {
                continue;
            };
            let Some(endpoint) = sse.data else {
                return Err(WorkerQuitReason::fatal(
                    "endpoint event without data",
                    "get the endpoint event",
                ));
            };
            break endpoint;
        };
        let post_uri: Arc<str> = format!(
            "{}/{}",
            self.config.uri.trim_end_matches("/"),
            endpoint.trim_start_matches("/")
        )
        .into();
        let mut retry_interval = self.config.retry_config.min_duration;
        let mut last_event_id = None;
        enum Event<W: Worker> {
            Sse(Option<Result<Sse, SseError>>),
            FromHandler(WorkerSendRequest<W>),
        }
        let quit_reason = loop {
            let event = tokio::select! {
                event = sse_stream.next() => {
                    Event::Sse(event)
                }
                _ = context.cancellation_token.cancelled() => {
                    tracing::debug!("cancelled");
                    break WorkerQuitReason::Cancelled;
                }
                from_handler = context.from_handler_rx.recv() => {
                    match from_handler {
                        Some(message) => Event::FromHandler(message),
                        None => break WorkerQuitReason::HandlerTerminated,
                    }
                }

            };
            let next_sse = match event {
                Event::Sse(Some(Ok(next_sse))) => next_sse,
                Event::Sse(Some(Err(e))) => {
                    tracing::warn!("sse stream error: {e}");
                    let mut retry_times = 0;
                    'retry_loop: loop {
                        tracing::debug!("sse stream error: {e}, retrying in {:?}", retry_interval);
                        tokio::time::sleep(retry_interval).await;
                        let retry_result = self
                            .client
                            .get_stream(self.config.uri.clone(), last_event_id.clone(), None)
                            .await;
                        retry_times += 1;
                        match retry_result {
                            Ok(new_stream) => {
                                sse_stream = new_stream;
                                break 'retry_loop;
                            }
                            Err(e) => {
                                if retry_times
                                    >= self.config.retry_config.max_times.unwrap_or(usize::MAX)
                                {
                                    tracing::error!(
                                        "sse stream error: {e}, max retry times reached"
                                    );
                                    return Err(WorkerQuitReason::fatal(
                                        e.to_string(),
                                        "sse stream error: max retry times reached",
                                    ));
                                } else {
                                    continue 'retry_loop;
                                }
                            }
                        }
                    }
                    continue;
                }
                Event::Sse(None) => {
                    tracing::debug!("sse stream terminated");
                    break WorkerQuitReason::HandlerTerminated;
                }
                Event::FromHandler(send_request) => {
                    let post_result = self
                        .client
                        .post_message(post_uri.clone(), send_request.message, None)
                        .await;
                    send_request.responder.send(post_result).ok();
                    continue;
                }
            };
            // set the retry interval
            if let Some(server_retry_interval) = next_sse.retry {
                retry_interval = retry_interval.min(Duration::from_millis(server_retry_interval));
            }

            if let Some(data) = next_sse.data {
                match serde_json::from_slice::<ServerJsonRpcMessage>(data.as_bytes()) {
                    Err(e) => tracing::warn!("failed to deserialize server message: {e}"),
                    Ok(message) => {
                        let yield_result = context.to_handler_tx.send(message).await;
                        if yield_result.is_err() {
                            tracing::trace!("streamable http transport worker dropped, exiting");
                            break WorkerQuitReason::Cancelled;
                        }
                    }
                };
            }

            if let Some(id) = next_sse.id {
                last_event_id = Some(id);
            }
        };
        Err(quit_reason)
    }
}

pub type SseClientTransport<C> = WorkerTransport<SseClientWorker<C>>;

impl<C: SseClient> SseClientTransport<C> {
    pub fn with_client(client: C, config: SseClientConfig) -> Self {
        let worker = SseClientWorker::new(client, config);
        WorkerTransport::spawn(worker)
    }
}
