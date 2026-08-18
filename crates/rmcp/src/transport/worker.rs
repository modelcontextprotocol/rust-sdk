use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use tokio_util::sync::CancellationToken;
use tracing::{Instrument, Level};

use super::{IntoTransport, Transport};
use crate::{
    model::{CancelledNotification, JsonRpcMessage, RequestId},
    service::{RxJsonRpcMessage, ServiceRole, TxJsonRpcMessage},
};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WorkerQuitReason<E> {
    #[error("Join error {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("Transport fatal {error}, when {context}")]
    Fatal {
        error: E,
        context: Cow<'static, str>,
    },
    #[error("Transport cancelled")]
    Cancelled,
    #[error("Transport closed")]
    TransportClosed,
    #[error("Handler terminated")]
    HandlerTerminated,
    #[error("Worker idle timeout after {}ms", _0.as_millis())]
    IdleTimeout(Duration),
}

impl<E: std::error::Error + Send + 'static> WorkerQuitReason<E> {
    pub fn fatal(error: E, context: impl Into<Cow<'static, str>>) -> Self {
        Self::Fatal {
            error,
            context: context.into(),
        }
    }
    pub fn fatal_context(context: impl Into<Cow<'static, str>>) -> impl FnOnce(E) -> Self {
        |e| Self::Fatal {
            error: e,
            context: context.into(),
        }
    }
}

pub trait Worker: Sized + Send + 'static {
    type Error: std::error::Error + Send + Sync + 'static;
    type Role: ServiceRole;
    fn err_closed() -> Self::Error;
    fn err_join(e: tokio::task::JoinError) -> Self::Error;
    fn run(
        self,
        context: WorkerContext<Self>,
    ) -> impl Future<Output = Result<(), WorkerQuitReason<Self::Error>>> + Send;
    fn config(&self) -> WorkerConfig {
        WorkerConfig::default()
    }
    /// Return true to send this message through the separate control queue.
    ///
    /// Workers that opt in must read [`WorkerContext::control_from_handler_rx`]
    /// and preserve any required ordering with ordinary messages.
    fn is_control_message(_message: &TxJsonRpcMessage<Self::Role>) -> bool {
        false
    }
    /// Return true to register outgoing requests for cancellation before they enter a queue.
    ///
    /// Workers that opt in must honor [`WorkerSendRequest::cancellation_token`].
    fn supports_request_cancellation() -> bool {
        false
    }
}

type RequestCancellations = Arc<Mutex<HashMap<RequestId, Arc<CancellationToken>>>>;

/// Keeps a request's cancellation token registered for a chosen lifetime.
pub(crate) struct RequestCancellationRegistration {
    id: RequestId,
    cancellation: Arc<CancellationToken>,
    pending: RequestCancellations,
}

impl RequestCancellationRegistration {
    fn new(id: RequestId, token: CancellationToken, pending: RequestCancellations) -> Self {
        let cancellation = Arc::new(token);
        pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.clone(), cancellation.clone());
        Self {
            id,
            cancellation,
            pending,
        }
    }

    /// Return the token kept alive by this registration.
    pub(crate) fn token(&self) -> CancellationToken {
        self.cancellation.as_ref().clone()
    }
}

impl Drop for RequestCancellationRegistration {
    fn drop(&mut self) {
        self.cancellation.cancel();
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        if pending
            .get(&self.id)
            .is_some_and(|current| Arc::ptr_eq(current, &self.cancellation))
        {
            pending.remove(&self.id);
        }
    }
}

#[non_exhaustive]
pub struct WorkerSendRequest<W: Worker> {
    pub message: TxJsonRpcMessage<W::Role>,
    pub responder: tokio::sync::oneshot::Sender<Result<(), W::Error>>,
    cancellation: Option<Arc<RequestCancellationRegistration>>,
    control_generation: u64,
}

impl<W: Worker> WorkerSendRequest<W> {
    /// Return the token registered before this request entered the send queue.
    ///
    /// This is present only for requests sent to a worker that enables
    /// [`Worker::supports_request_cancellation`]. It is not sent over the wire.
    pub fn cancellation_token(&self) -> Option<CancellationToken> {
        self.cancellation
            .as_deref()
            .map(RequestCancellationRegistration::token)
    }

    /// Keep the same cancellation registration alive after the POST completes.
    #[cfg(feature = "transport-streamable-http-client")]
    pub(crate) fn cancellation_registration(&self) -> Option<Arc<RequestCancellationRegistration>> {
        self.cancellation.clone()
    }

    /// Return the control generation when the send was created.
    pub fn control_generation(&self) -> u64 {
        self.control_generation
    }
}

pub struct WorkerTransport<W: Worker> {
    rx: tokio::sync::mpsc::Receiver<RxJsonRpcMessage<W::Role>>,
    send_service: tokio::sync::mpsc::Sender<WorkerSendRequest<W>>,
    control_send_service: tokio::sync::mpsc::Sender<WorkerSendRequest<W>>,
    request_cancellations: RequestCancellations,
    control_generation: Arc<AtomicU64>,
    join_handle: Option<tokio::task::JoinHandle<Result<(), WorkerQuitReason<W::Error>>>>,
    _drop_guard: tokio_util::sync::DropGuard,
    ct: CancellationToken,
}

#[non_exhaustive]
pub struct WorkerConfig {
    pub name: Option<String>,
    pub channel_buffer_capacity: usize,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            name: None,
            channel_buffer_capacity: 16,
        }
    }
}
#[non_exhaustive]
pub enum WorkerAdapter {}

impl<W: Worker> IntoTransport<W::Role, W::Error, WorkerAdapter> for W {
    fn into_transport(self) -> impl Transport<W::Role, Error = W::Error> + 'static {
        WorkerTransport::spawn(self)
    }
}

impl<W: Worker> WorkerTransport<W> {
    pub fn cancel_token(&self) -> CancellationToken {
        self.ct.clone()
    }
    pub fn spawn(worker: W) -> Self {
        Self::spawn_with_ct(worker, CancellationToken::new())
    }
    pub fn spawn_with_ct(worker: W, transport_task_ct: CancellationToken) -> Self {
        let config = worker.config();
        let worker_name = config.name;
        let (to_transport_tx, from_handler_rx) =
            tokio::sync::mpsc::channel::<WorkerSendRequest<W>>(config.channel_buffer_capacity);
        let (control_to_transport_tx, control_from_handler_rx) =
            tokio::sync::mpsc::channel::<WorkerSendRequest<W>>(config.channel_buffer_capacity);
        let (to_handler_tx, from_transport_rx) =
            tokio::sync::mpsc::channel::<RxJsonRpcMessage<W::Role>>(config.channel_buffer_capacity);
        let request_cancellations = RequestCancellations::default();
        let control_generation = Arc::new(AtomicU64::new(0));
        let context = WorkerContext {
            to_handler_tx,
            from_handler_rx,
            control_from_handler_rx,
            control_generation: control_generation.clone(),
            cancellation_token: transport_task_ct.clone(),
        };

        let join_handle = tokio::spawn(async move {
            worker
                .run(context)
                .instrument(tracing::span!(
                    Level::TRACE,
                    "transport_worker",
                    name = worker_name,
                ))
                .await
                .inspect_err(|e| match e {
                    WorkerQuitReason::Cancelled
                    | WorkerQuitReason::TransportClosed
                    | WorkerQuitReason::HandlerTerminated
                    | WorkerQuitReason::IdleTimeout(_) => {
                        tracing::debug!("worker quit with reason: {:?}", e);
                    }
                    WorkerQuitReason::Join(e) => {
                        tracing::error!("worker quit with join error: {:?}", e);
                    }
                    WorkerQuitReason::Fatal { error, context } => {
                        tracing::error!("worker quit with fatal: {error}, when {context}");
                    }
                })
                .inspect(|_| {
                    tracing::debug!("worker quit");
                })
        });
        Self {
            rx: from_transport_rx,
            send_service: to_transport_tx,
            control_send_service: control_to_transport_tx,
            request_cancellations,
            control_generation,
            join_handle: Some(join_handle),
            ct: transport_task_ct.clone(),
            _drop_guard: transport_task_ct.drop_guard(),
        }
    }
}

#[non_exhaustive]
pub struct SendRequest<W: Worker> {
    pub message: TxJsonRpcMessage<W::Role>,
    pub responder: tokio::sync::oneshot::Sender<RxJsonRpcMessage<W::Role>>,
}

#[non_exhaustive]
pub struct WorkerContext<W: Worker> {
    pub to_handler_tx: tokio::sync::mpsc::Sender<RxJsonRpcMessage<W::Role>>,
    pub from_handler_rx: tokio::sync::mpsc::Receiver<WorkerSendRequest<W>>,
    /// Messages selected by [`Worker::is_control_message`].
    pub control_from_handler_rx: tokio::sync::mpsc::Receiver<WorkerSendRequest<W>>,
    pub cancellation_token: CancellationToken,
    control_generation: Arc<AtomicU64>,
}

impl<W: Worker> WorkerContext<W> {
    /// Return the generation assigned to newly created control sends.
    pub fn control_generation(&self) -> u64 {
        self.control_generation.load(Ordering::SeqCst)
    }

    /// Advance the generation so the worker can reject older control sends.
    pub fn advance_control_generation(&self) -> u64 {
        self.control_generation
            .fetch_add(1, Ordering::SeqCst)
            .wrapping_add(1)
    }

    pub async fn send_to_handler(
        &mut self,
        item: RxJsonRpcMessage<W::Role>,
    ) -> Result<(), WorkerQuitReason<W::Error>> {
        self.to_handler_tx
            .send(item)
            .await
            .map_err(|_| WorkerQuitReason::HandlerTerminated)
    }

    pub async fn recv_from_handler(
        &mut self,
    ) -> Result<WorkerSendRequest<W>, WorkerQuitReason<W::Error>> {
        self.from_handler_rx
            .recv()
            .await
            .ok_or(WorkerQuitReason::HandlerTerminated)
    }
}

impl<W: Worker> Transport<W::Role> for WorkerTransport<W> {
    type Error = W::Error;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<W::Role>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let control_generation = self.control_generation.load(Ordering::SeqCst);
        let registration = if W::supports_request_cancellation() {
            match &item {
                JsonRpcMessage::Request(request) => {
                    Some(Arc::new(RequestCancellationRegistration::new(
                        request.id.clone(),
                        self.ct.child_token(),
                        self.request_cancellations.clone(),
                    )))
                }
                JsonRpcMessage::Notification(notification) => {
                    let cancelled: Result<CancelledNotification, _> =
                        notification.notification.clone().try_into();
                    if let Ok(cancelled) = cancelled
                        && let Some(id) = cancelled.params.request_id.as_ref()
                    {
                        let pending = self
                            .request_cancellations
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner);
                        if let Some(cancellation) = pending.get(id).or_else(|| {
                            id.numeric_string_value()
                                .and_then(|id| pending.get(&RequestId::Number(id)))
                        }) {
                            // Signal cancellation even if the control queue is full.
                            cancellation.cancel();
                        }
                    }
                    None
                }
                _ => None,
            }
        } else {
            None
        };
        let tx = if W::is_control_message(&item) {
            self.control_send_service.clone()
        } else {
            self.send_service.clone()
        };
        let cancellation_guard = registration
            .as_ref()
            .map(|registration| registration.token().drop_guard());
        let (responder, receiver) = tokio::sync::oneshot::channel();
        let request = WorkerSendRequest {
            message: item,
            responder,
            cancellation: registration,
            control_generation,
        };
        async move {
            tx.send(request).await.map_err(|_| W::err_closed())?;
            receiver.await.map_err(|_| W::err_closed())??;
            if let Some(guard) = cancellation_guard {
                let _ = guard.disarm();
            }
            Ok(())
        }
    }
    async fn receive(&mut self) -> Option<RxJsonRpcMessage<W::Role>> {
        self.rx.recv().await
    }
    async fn close(&mut self) -> Result<(), Self::Error> {
        if let Some(handle) = self.join_handle.take() {
            self.ct.cancel();
            let _quit_reason = handle.await.map_err(W::err_join)?;
            Ok(())
        } else {
            Ok(())
        }
    }
}
