use std::{borrow::Cow, sync::Arc, time::Duration};

use futures::{StreamExt, stream::BoxStream};
pub use sse_stream::Error as SseError;
use sse_stream::Sse;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use super::common::sse::SseRetryConfig;
use crate::{
    RoleClient,
    model::{ClientJsonRpcMessage, ServerJsonRpcMessage},
    transport::worker::{Worker, WorkerQuitReason, WorkerSendRequest, WorkerTransport},
};

type BoxedSseStream = BoxStream<'static, Result<Sse, SseError>>;

#[derive(Error, Debug)]
pub enum StreamableHttpError<E: std::error::Error + Send + Sync + 'static> {
    #[error("SSE error: {0}")]
    Sse(#[from] SseError),
    #[error("Io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Client error: {0}")]
    Client(E),
    #[error("unexpected end of stream")]
    UnexpectedEndOfStream,
    #[error("unexpected server response: {0}")]
    UnexpectedServerResponse(Cow<'static, str>),
    #[error("Unexpected content type: {0:?}")]
    UnexpectedContentType(Option<String>),
    #[error("Server does not support SSE")]
    SeverDoesNotSupportSse,
    #[error("Server does not support delete session")]
    SeverDoesNotSupportDeleteSession,
    #[error("Tokio join error: {0}")]
    TokioJoinError(#[from] tokio::task::JoinError),
    #[error("Deserialize error: {0}")]
    Deserialize(#[from] serde_json::Error),
    #[error("Transport channel closed")]
    TransportChannelClosed,
    #[cfg(feature = "__auth")]
    #[cfg_attr(docsrs, doc(cfg(feature = "__auth")))]
    #[error("Auth error: {0}")]
    Auth(#[from] crate::transport::auth::AuthError),
}

impl From<reqwest::Error> for StreamableHttpError<reqwest::Error> {
    fn from(e: reqwest::Error) -> Self {
        StreamableHttpError::Client(e)
    }
}

pub enum StreamableHttpPostResponse {
    Accepted,
    Json(StreamableHttpPostJsonResponse),
    Sse(BoxedSseStream),
}

pub struct StreamableHttpPostJsonResponse {
    pub message: ServerJsonRpcMessage,
    pub session_id: Option<String>,
}

impl StreamableHttpPostResponse {
    pub fn expect_json<E>(self) -> Result<StreamableHttpPostJsonResponse, StreamableHttpError<E>>
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        match self {
            Self::Json(message) => Ok(message),
            _ => Err(StreamableHttpError::UnexpectedServerResponse(
                "expected json".into(),
            )),
        }
    }

    pub fn expect_accepted<E>(self) -> Result<(), StreamableHttpError<E>>
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        match self {
            Self::Accepted => Ok(()),
            _ => Err(StreamableHttpError::UnexpectedServerResponse(
                "expected accepted".into(),
            )),
        }
    }
}

pub trait StreamableHttpClient: Clone + Send + 'static {
    type Error: std::error::Error + Send + Sync + 'static;
    fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
    ) -> impl Future<Output = Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>>>
    + Send
    + '_;
    fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
    ) -> impl Future<Output = Result<(), StreamableHttpError<Self::Error>>> + Send + '_;
    fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
    ) -> impl Future<
        Output = Result<
            BoxStream<'static, Result<Sse, SseError>>,
            StreamableHttpError<Self::Error>,
        >,
    > + Send
    + '_;
}

pub struct RetryConfig {
    pub max_times: Option<usize>,
    pub min_duration: Duration,
}
#[derive(Debug, Clone, Default)]
pub struct StreamableHttpClientWorker<C: StreamableHttpClient> {
    pub client: C,
    pub config: StreamableHttpClientTransportConfig,
}

impl<C: StreamableHttpClient + Default> StreamableHttpClientWorker<C> {
    pub fn new_simple(url: impl Into<Arc<str>>) -> Self {
        Self {
            client: C::default(),
            config: StreamableHttpClientTransportConfig {
                uri: url.into(),
                retry_config: SseRetryConfig::default(),
                channel_buffer_capacity: 16,
            },
        }
    }
}

impl<C: StreamableHttpClient> StreamableHttpClientWorker<C> {
    pub fn new(client: C, config: StreamableHttpClientTransportConfig) -> Self {
        Self { client, config }
    }
}

impl<C: StreamableHttpClient> StreamableHttpClientWorker<C> {
    async fn execute_sse_stream(
        self,
        sse_stream: BoxedSseStream,
        sse_worker_tx: tokio::sync::mpsc::Sender<ServerJsonRpcMessage>,
        session_id: Arc<str>,
        ct: CancellationToken,
    ) -> Result<(), StreamableHttpError<C::Error>> {
        let mut sse_stream = sse_stream;
        let mut retry_interval = self.config.retry_config.min_duration;
        let mut last_event_id = None;
        loop {
            let event = tokio::select! {
                event = sse_stream.next() => {
                    event
                }
                _ = ct.cancelled() => {
                    tracing::debug!("cancelled");
                    break;
                }
            };
            let next_sse = match event {
                Some(Ok(next_sse)) => next_sse,
                Some(Err(e)) => {
                    tracing::warn!("sse stream error: {e}");
                    let mut retry_times = 0;
                    'retry_loop: loop {
                        tracing::debug!("sse stream error: {e}, retrying in {:?}", retry_interval);
                        tokio::time::sleep(retry_interval).await;
                        let retry_result = self
                            .client
                            .get_stream(
                                self.config.uri.clone(),
                                session_id.clone(),
                                last_event_id.clone(),
                                None,
                            )
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
                                    return Err(e);
                                } else {
                                    continue 'retry_loop;
                                }
                            }
                        }
                    }
                    continue;
                }
                None => {
                    tracing::debug!("sse stream terminated");
                    break;
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
                        let yield_result = sse_worker_tx.send(message).await;
                        if yield_result.is_err() {
                            tracing::trace!("streamable http transport worker dropped, exiting");
                            break;
                        }
                    }
                };
            }

            if let Some(id) = next_sse.id {
                last_event_id = Some(id);
            }
        }
        Ok(())
    }
}

impl<C: StreamableHttpClient> Worker for StreamableHttpClientWorker<C> {
    type Role = RoleClient;
    type Error = StreamableHttpError<C::Error>;
    fn err_closed() -> Self::Error {
        StreamableHttpError::TransportChannelClosed
    }
    fn err_join(e: tokio::task::JoinError) -> Self::Error {
        StreamableHttpError::TokioJoinError(e)
    }
    fn config(&self) -> super::worker::WorkerConfig {
        super::worker::WorkerConfig {
            name: Some("StreamableHttpClientWorker".into()),
            channel_buffer_capacity: self.config.channel_buffer_capacity,
        }
    }
    async fn run(
        self,
        mut context: super::worker::WorkerContext<Self>,
    ) -> Result<(), WorkerQuitReason> {
        let channel_buffer_capacity = self.config.channel_buffer_capacity;
        let (sse_worker_tx, mut sse_worker_rx) =
            tokio::sync::mpsc::channel::<ServerJsonRpcMessage>(channel_buffer_capacity);
        // let super::worker::WorkerContext {
        //     to_handler_tx,
        //     mut from_handler_rx,
        //     cancellation_token: transport_task_ct,
        // } = context;
        let config = self.config.clone();
        let transport_task_ct = context.cancellation_token.clone();
        let _drop_guard = transport_task_ct.clone().drop_guard();
        let WorkerSendRequest {
            responder,
            message: initialize_request,
        } = context.recv_from_handler().await?;
        let _ = responder.send(Ok(()));
        let StreamableHttpPostJsonResponse {
            session_id,
            message,
        } = self
            .client
            .post_message(config.uri.clone(), initialize_request, None, None)
            .await
            .map_err(WorkerQuitReason::fatal_context("send initialize request"))?
            .expect_json::<Self::Error>()
            .map_err(WorkerQuitReason::fatal_context(
                "process initialize response",
            ))?;
        let Some(session_id) = session_id else {
            return Err(WorkerQuitReason::fatal(
                "missing session id in initialize response",
                "process initialize response",
            ));
        };
        let session_id: Arc<str> = session_id.into();

        // delete session when drop guard is dropped
        {
            let ct = transport_task_ct.clone();
            let client = self.client.clone();
            let session_id = session_id.clone();
            let url = config.uri.clone();
            tokio::spawn(async move {
                ct.cancelled().await;
                let delete_session_result =
                    client.delete_session(url, session_id.clone(), None).await;
                match delete_session_result {
                    Ok(_) => {
                        tracing::info!(session_id = session_id.as_ref(), "delete session success")
                    }
                    Err(StreamableHttpError::SeverDoesNotSupportDeleteSession) => {
                        tracing::info!(
                            session_id = session_id.as_ref(),
                            "server doesn't support delete session"
                        )
                    }
                    Err(e) => {
                        tracing::error!(
                            session_id = session_id.as_ref(),
                            "fail to delete session: {e}"
                        );
                    }
                };
            });
        }

        context.send_to_handler(message).await?;
        let initialized_notification = context.recv_from_handler().await?;
        // expect a initialized response
        self.client
            .post_message(
                config.uri.clone(),
                initialized_notification.message,
                Some(session_id.clone()),
                None,
            )
            .await
            .map_err(WorkerQuitReason::fatal_context(
                "send initialized notification",
            ))?
            .expect_accepted::<Self::Error>()
            .map_err(WorkerQuitReason::fatal_context(
                "process initialized notification response",
            ))?;
        let _ = initialized_notification.responder.send(Ok(()));
        enum Event<W: Worker, E: std::error::Error + Send + Sync + 'static> {
            ClientMessage(WorkerSendRequest<W>),
            ServerMessage(ServerJsonRpcMessage),
            StreamResult(Result<(), StreamableHttpError<E>>),
        }
        let mut streams = tokio::task::JoinSet::new();
        match self
            .client
            .get_stream(config.uri.clone(), session_id.clone(), None, None)
            .await
        {
            Ok(stream) => {
                streams.spawn(self.clone().execute_sse_stream(
                    stream,
                    sse_worker_tx.clone(),
                    session_id.clone(),
                    transport_task_ct.child_token(),
                ));
                tracing::debug!("got common stream");
            }
            Err(StreamableHttpError::SeverDoesNotSupportSse) => {}
            Err(e) => {
                // fail to get common stream
                tracing::error!("fail to get common stream: {e}");
                return Err(WorkerQuitReason::fatal(
                    "fail to get general purpose event stream",
                    "get general purpose event stream",
                ));
            }
        }
        loop {
            let event = tokio::select! {
                message = context.recv_from_handler() => {
                    let message = message?;
                    Event::ClientMessage(message)
                },
                message = sse_worker_rx.recv() => {
                    let Some(message) = message else {
                        tracing::trace!("transport dropped, exiting");
                        return Err(WorkerQuitReason::HandlerTerminated);
                    };
                    Event::ServerMessage(message)
                },
                terminated_stream = streams.join_next(), if !streams.is_empty() => {
                    match terminated_stream {
                        Some(result) => {
                            Event::StreamResult(result.map_err(StreamableHttpError::TokioJoinError).and_then(std::convert::identity))
                        }
                        None => {
                            continue
                        }
                    }
                }
            };
            match event {
                Event::ClientMessage(send_request) => {
                    let WorkerSendRequest { message, responder } = send_request;
                    let response = self
                        .client
                        .post_message(config.uri.clone(), message, Some(session_id.clone()), None)
                        .await;
                    let send_result = match response {
                        Err(e) => Err(e),
                        Ok(StreamableHttpPostResponse::Accepted) => {
                            tracing::trace!("client message accepted");
                            Ok(())
                        }
                        Ok(StreamableHttpPostResponse::Json(message)) => {
                            context.send_to_handler(message.message).await?;
                            Ok(())
                        }
                        Ok(StreamableHttpPostResponse::Sse(stream)) => {
                            streams.spawn(self.clone().execute_sse_stream(
                                stream,
                                sse_worker_tx.clone(),
                                session_id.clone(),
                                transport_task_ct.child_token(),
                            ));
                            tracing::trace!("got new sse stream");
                            Ok(())
                        }
                    };
                    let _ = responder.send(send_result);
                }
                Event::ServerMessage(json_rpc_message) => {
                    // send the message to the handler
                    context.send_to_handler(json_rpc_message).await?;
                }
                Event::StreamResult(result) => {
                    if result.is_err() {
                        tracing::warn!(
                            "sse client event stream terminated with error: {:?}",
                            result
                        );
                    }
                }
            }
        }
    }
}

pub type StreamableHttpClientTransport<C> = WorkerTransport<StreamableHttpClientWorker<C>>;

impl<C: StreamableHttpClient> StreamableHttpClientTransport<C> {
    pub fn with_client(client: C, config: StreamableHttpClientTransportConfig) -> Self {
        let worker = StreamableHttpClientWorker::new(client, config);
        WorkerTransport::spawn(worker)
    }
}
#[derive(Debug, Clone)]
pub struct StreamableHttpClientTransportConfig {
    pub uri: Arc<str>,
    pub retry_config: SseRetryConfig,
    pub channel_buffer_capacity: usize,
}

impl StreamableHttpClientTransportConfig {
    pub fn with_uri(uri: impl Into<Arc<str>>) -> Self {
        Self {
            uri: uri.into(),
            ..Default::default()
        }
    }
}

impl Default for StreamableHttpClientTransportConfig {
    fn default() -> Self {
        Self {
            uri: "localhost".into(),
            retry_config: SseRetryConfig::default(),
            channel_buffer_capacity: 16,
        }
    }
}
