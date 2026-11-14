use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{
        Arc, Mutex, PoisonError, Weak,
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
    /// The transport's startup handshake failed with the typed error.
    ///
    /// Distinct from [`Fatal`](Self::Fatal): a startup failure is an ordinary,
    /// often retryable lifecycle event (peer unreachable, TLS failure, an auth
    /// challenge), not an internal fault. The real typed error is retained here
    /// rather than replaced by a synthetic placeholder, so the service loop and
    /// late observers can see the actual cause.
    ///
    /// The error is shared via `Arc` so it can be delivered to the in-flight
    /// `send()` ack *and* retained here — a single typed value would be lost if
    /// the ack's receiver is dropped (its own timeout) after a successful queue.
    #[error("Startup failed {error}, when {context}")]
    StartupFailed {
        error: Arc<E>,
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
    pub fn startup_failed(error: E, context: impl Into<Cow<'static, str>>) -> Self {
        Self::StartupFailed {
            error: Arc::new(error),
            context: context.into(),
        }
    }
    pub fn startup_failed_context(context: impl Into<Cow<'static, str>>) -> impl FnOnce(E) -> Self {
        |e| Self::StartupFailed {
            error: Arc::new(e),
            context: context.into(),
        }
    }
}

pub trait Worker: Sized + Send + 'static {
    type Error: std::error::Error + Send + Sync + 'static;
    type Role: ServiceRole;
    fn err_closed() -> Self::Error;
    /// Whether this is the synthetic error produced when the worker's channel
    /// closes, rather than a real transport failure delivered by the worker.
    fn is_closed_error(_error: &Self::Error) -> bool {
        false
    }
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

type RequestCancellations = Arc<Mutex<HashMap<RequestId, Weak<RequestCancellationRegistration>>>>;

/// Keeps a request's cancellation token registered for a chosen lifetime.
pub(crate) struct RequestCancellationRegistration {
    id: RequestId,
    lifetime: CancellationToken,
    cancellation: CancellationToken,
    pending: RequestCancellations,
}

impl RequestCancellationRegistration {
    fn new(id: RequestId, token: CancellationToken, pending: RequestCancellations) -> Arc<Self> {
        let registration = Arc::new(Self {
            id: id.clone(),
            cancellation: token.child_token(),
            lifetime: token,
            pending,
        });
        registration
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, Arc::downgrade(&registration));
        registration
    }

    /// Return the token kept alive by this registration.
    pub(crate) fn token(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    /// Return the token cancelled when the request lifetime ends.
    pub(crate) fn lifetime_token(&self) -> CancellationToken {
        self.lifetime.clone()
    }
}

impl Drop for RequestCancellationRegistration {
    fn drop(&mut self) {
        self.lifetime.cancel();
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        if pending
            .get(&self.id)
            .is_some_and(|current| std::ptr::eq(current.as_ptr(), self))
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
    /// Keep this request alive while its work is active. Cloning the token does
    /// not keep its cancellation registration alive.
    pub fn cancellation_token(&self) -> Option<CancellationToken> {
        self.cancellation
            .as_deref()
            .map(RequestCancellationRegistration::token)
    }

    /// Keep the same cancellation registration alive after the send completes.
    #[cfg(feature = "transport-streamable-http-client")]
    pub(crate) fn cancellation_registration(&self) -> Option<Arc<RequestCancellationRegistration>> {
        self.cancellation.clone()
    }

    /// Return the local generation captured when [`Transport::send`] created its future.
    ///
    /// This happens before polling or queue admission. The value is not sent over
    /// the wire; the worker decides whether a message from an older generation is valid.
    /// It identifies the outbound send, not the session that started an inbound handler.
    pub fn control_generation(&self) -> u64 {
        self.control_generation
    }
}

/// A retained worker terminal reason, shared via `Arc` so it can outlive the
/// single fire-and-forget ack delivery window.
///
/// Boxed into [`DynamicTransportError`](crate::transport::DynamicTransportError)
/// by [`Transport::quit_status`].
/// Callers can downcast to this type to recover the `Arc<WorkerQuitReason<E>>`
/// (and thus the typed `E`), or walk [`std::error::Error::source`] to reach the
/// inner typed error directly — preserving source-only data such as an auth challenge.
#[derive(Debug)]
#[non_exhaustive]
pub struct RetainedQuitReason<E>(pub Arc<WorkerQuitReason<E>>);

impl<E> RetainedQuitReason<E> {
    /// The typed error behind an abnormal quit, if this reason carries one.
    ///
    /// Startup failures share their error through an `Arc`; fatal errors own
    /// it. Normal shutdown reasons (cancel, clean close, handler termination,
    /// idle timeout) carry no diagnostic error and return `None`.
    ///
    /// This backs [`DynamicTransportError::downcast_ref`], letting callers
    /// reach the same `&E` on the retained path as on the live-ack path.
    ///
    /// [`DynamicTransportError::downcast_ref`]: crate::transport::DynamicTransportError::downcast_ref
    pub fn startup_error(&self) -> Option<&E> {
        match self.0.as_ref() {
            WorkerQuitReason::StartupFailed { error, .. } => Some(error.as_ref()),
            WorkerQuitReason::Fatal { error, .. } => Some(error),
            _ => None,
        }
    }
}

impl<E: std::error::Error + Send + 'static> std::fmt::Display for RetainedQuitReason<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self.0.as_ref(), f)
    }
}

impl<E: std::error::Error + Send + Sync + 'static> std::error::Error for RetainedQuitReason<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self.0.as_ref() {
            WorkerQuitReason::StartupFailed { error, .. } => Some(error.as_ref()),
            WorkerQuitReason::Fatal { error, .. } => Some(error),
            WorkerQuitReason::Join(e) => Some(e),
            _ => None,
        }
    }
}

type WorkerJoinHandle<E> = tokio::task::JoinHandle<Result<(), Arc<WorkerQuitReason<E>>>>;

pub struct WorkerTransport<W: Worker> {
    rx: tokio::sync::mpsc::Receiver<RxJsonRpcMessage<W::Role>>,
    send_service: tokio::sync::mpsc::Sender<WorkerSendRequest<W>>,
    control_send_service: tokio::sync::mpsc::Sender<WorkerSendRequest<W>>,
    request_cancellations: RequestCancellations,
    control_generation: Arc<AtomicU64>,
    join_handle: Option<WorkerJoinHandle<W::Error>>,
    /// Retained terminal status of the worker, written exactly once when its
    /// task quits. Lets an observer whose own timeout has already fired still
    /// learn why the worker died, rather than relying on a delivery window.
    quit_status: tokio::sync::watch::Receiver<Option<Arc<WorkerQuitReason<W::Error>>>>,
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

        // Retained terminal status: written once when the worker task finishes,
        // so the quit reason is not lost when no `Transport::close()` is pending
        // at the moment the worker dies. The reason is shared via `Arc` so both
        // the join handle (consumed by close()) and late observers can read it.
        let (quit_status_tx, quit_status) = tokio::sync::watch::channel(None);

        let join_handle = tokio::spawn(async move {
            // Hold the input channel open until the terminal status is
            // published: `run()` drops its `WorkerContext` (and thus
            // `to_handler_tx`) on return, so without this a concurrently polled
            // `receive()` could observe EOF and read `quit_status()` as `None`
            // before the watch write lands. Keeping a clone alive until after
            // the write orders EOF after the status is visible.
            let keep_input_open = context.to_handler_tx.clone();
            let result = worker
                .run(context)
                .instrument(tracing::span!(
                    Level::TRACE,
                    "transport_worker",
                    name = worker_name,
                ))
                .await;
            match &result {
                Ok(()) => {
                    tracing::debug!("worker quit");
                }
                Err(e) => match e {
                    WorkerQuitReason::Cancelled
                    | WorkerQuitReason::TransportClosed
                    | WorkerQuitReason::HandlerTerminated
                    | WorkerQuitReason::IdleTimeout(_) => {
                        tracing::debug!("worker quit with reason: {:?}", e);
                    }
                    WorkerQuitReason::Join(e) => {
                        tracing::error!("worker quit with join error: {:?}", e);
                    }
                    WorkerQuitReason::StartupFailed { error, context } => {
                        // A startup failure is an ordinary lifecycle event
                        // (peer unreachable, TLS failure, auth challenge), not
                        // an internal fault — log at warn, not error.
                        tracing::warn!("worker startup failed: {error}, when {context}");
                    }
                    WorkerQuitReason::Fatal { error, context } => {
                        tracing::error!("worker quit with fatal: {error}, when {context}");
                    }
                },
            }
            // Retain the terminal reason for late observers; the same value is
            // shared with the join handle so close() can report join errors.
            let quit = match result {
                Ok(()) => {
                    let _ = quit_status_tx.send(None);
                    Ok(())
                }
                Err(e) => {
                    let shared = Arc::new(e);
                    let _ = quit_status_tx.send(Some(shared.clone()));
                    Err(shared)
                }
            };
            // Publish the status before the input channel closes.
            drop(keep_input_open);
            quit
        });
        Self {
            rx: from_transport_rx,
            send_service: to_transport_tx,
            control_send_service: control_to_transport_tx,
            request_cancellations,
            control_generation,
            join_handle: Some(join_handle),
            quit_status,
            ct: transport_task_ct.clone(),
            _drop_guard: transport_task_ct.drop_guard(),
        }
    }

    fn cancel_request_from_notification(
        &self,
        notification: &<W::Role as ServiceRole>::Not,
    ) -> Option<Arc<RequestCancellationRegistration>> {
        let cancelled: CancelledNotification = notification.clone().try_into().ok()?;
        let id = cancelled.params.request_id.as_ref()?;
        let target = {
            let pending = self
                .request_cancellations
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            pending.get(id).and_then(Weak::upgrade)
        }?;
        // Signal cancellation even if the control queue is full.
        target.token().cancel();
        Some(target)
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
    /// Return the local generation that newly created sends will capture.
    ///
    /// Workers may use this value to check messages from an earlier connection
    /// or session. The generic transport does not check it automatically.
    pub fn control_generation(&self) -> u64 {
        self.control_generation.load(Ordering::SeqCst)
    }

    /// Advance the local generation, wrapping at [`u64::MAX`], and return its new value.
    ///
    /// Only subsequent calls to [`Transport::send`] capture the new value.
    /// Advancing does not drain queues, cancel work, or reject older messages;
    /// the worker is responsible for those actions.
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
        let mut cancellation_target = None;
        let registration = if W::supports_request_cancellation() {
            match &item {
                JsonRpcMessage::Request(request) => Some(RequestCancellationRegistration::new(
                    request.id.clone(),
                    self.ct.child_token(),
                    self.request_cancellations.clone(),
                )),
                JsonRpcMessage::Notification(notification) => {
                    cancellation_target =
                        self.cancel_request_from_notification(&notification.notification);
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
            .map(|registration| registration.lifetime_token().drop_guard());
        let target_guard = cancellation_target
            .as_ref()
            .map(|target| target.lifetime_token().drop_guard());
        let (responder, receiver) = tokio::sync::oneshot::channel();
        let request = WorkerSendRequest {
            message: item,
            responder,
            cancellation: registration,
            control_generation,
        };
        async move {
            // Keep the stream alive until its cancellation is handled or abandoned.
            let _cancellation_target = cancellation_target;
            let _target_guard = target_guard;
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
            // Await worker termination; the quit reason itself is retained in
            // `quit_status` rather than moved out, so late observers can still
            // read it after close() returns.
            let _ = handle.await.map_err(W::err_join)?;
            Ok(())
        } else {
            Ok(())
        }
    }

    fn is_closed_error(error: &Self::Error) -> bool {
        W::is_closed_error(error)
    }

    fn supports_quit_status(&self) -> bool {
        true
    }

    fn quit_status(&self) -> Option<crate::transport::DynamicTransportError> {
        let guard = self.quit_status.borrow();
        let reason = guard.as_ref()?;
        // Only report a cause for abnormal termination. Normal shutdown paths
        // (cancel, clean transport close, handler termination, idle timeout)
        // carry no diagnostic value and must not masquerade as a `Transport`
        // quit reason on the service side.
        match reason.as_ref() {
            WorkerQuitReason::StartupFailed { .. }
            | WorkerQuitReason::Fatal { .. }
            | WorkerQuitReason::Join(_) => {}
            WorkerQuitReason::Cancelled
            | WorkerQuitReason::TransportClosed
            | WorkerQuitReason::HandlerTerminated
            | WorkerQuitReason::IdleTimeout(_) => return None,
        }
        // Retain the typed reason (not a stringified copy) so callers can still
        // downcast to `WorkerQuitReason<W::Error>` and reach source-only data
        // such as an auth challenge. The wrapper's `source()` exposes the inner
        // typed error for source-chain walks like `is_authorization_required`.
        Some(crate::transport::DynamicTransportError::from_parts(
            Self::name(),
            std::any::TypeId::of::<Self>(),
            Box::new(RetainedQuitReason(reason.clone())),
        ))
    }
}

#[cfg(all(test, feature = "client"))]
mod tests {
    use std::io;

    use super::*;
    use crate::{model::ClientJsonRpcMessage, service::RoleClient};

    struct TestWorker(tokio::sync::oneshot::Sender<WorkerContext<Self>>);

    impl Worker for TestWorker {
        type Error = io::Error;
        type Role = RoleClient;

        fn err_closed() -> Self::Error {
            io::Error::other("worker closed")
        }

        fn err_join(error: tokio::task::JoinError) -> Self::Error {
            io::Error::other(error)
        }

        async fn run(
            self,
            context: WorkerContext<Self>,
        ) -> Result<(), WorkerQuitReason<Self::Error>> {
            let cancellation = context.cancellation_token.clone();
            self.0
                .send(context)
                .map_err(|_| WorkerQuitReason::HandlerTerminated)?;
            cancellation.cancelled().await;
            Ok(())
        }

        fn is_control_message(message: &ClientJsonRpcMessage) -> bool {
            matches!(message, JsonRpcMessage::Notification(_))
        }

        fn supports_request_cancellation() -> bool {
            true
        }
    }

    fn cancellation_message(id: RequestId) -> ClientJsonRpcMessage {
        serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/cancelled",
            "params": { "requestId": id },
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn cancellation_matches_request_id_exactly() {
        let (context_tx, context_rx) = tokio::sync::oneshot::channel();
        let mut transport = WorkerTransport::spawn(TestWorker(context_tx));
        let _context = context_rx.await.unwrap();
        let registration = RequestCancellationRegistration::new(
            RequestId::Number(7),
            CancellationToken::new(),
            transport.request_cancellations.clone(),
        );

        drop(transport.send(cancellation_message(RequestId::String("7".into()))));
        assert!(!registration.token().is_cancelled());

        drop(transport.send(cancellation_message(RequestId::Number(7))));
        assert!(registration.token().is_cancelled());
        transport.close().await.unwrap();
    }

    #[tokio::test]
    async fn abandoned_cancellation_send_ends_request_lifetime() {
        let (context_tx, context_rx) = tokio::sync::oneshot::channel();
        let mut transport = WorkerTransport::spawn(TestWorker(context_tx));
        let mut context = context_rx.await.unwrap();

        for admitted in [false, true] {
            let id = RequestId::Number(7);
            let registration = RequestCancellationRegistration::new(
                id.clone(),
                CancellationToken::new(),
                transport.request_cancellations.clone(),
            );
            let lifetime = registration.lifetime_token();
            let weak = Arc::downgrade(&registration);
            let mut send = Box::pin(transport.send(cancellation_message(id)));
            assert!(registration.token().is_cancelled());
            assert!(!lifetime.is_cancelled());

            let queued = if admitted {
                assert!(futures::poll!(send.as_mut()).is_pending());
                Some(context.control_from_handler_rx.recv().await.unwrap())
            } else {
                None
            };
            drop(send);

            // An open stream can still own the registration after cancellation is abandoned.
            assert!(lifetime.is_cancelled());
            assert!(weak.upgrade().is_some());
            drop(registration);
            assert!(weak.upgrade().is_none());
            assert!(queued.is_none_or(|request| request.responder.is_closed()));
            assert!(transport.request_cancellations.lock().unwrap().is_empty());
        }
        transport.close().await.unwrap();
    }

    #[test]
    fn dropping_an_old_registration_preserves_a_reused_id() {
        let pending = RequestCancellations::default();
        let id = RequestId::Number(7);
        let old = RequestCancellationRegistration::new(
            id.clone(),
            CancellationToken::new(),
            pending.clone(),
        );
        let current = RequestCancellationRegistration::new(
            id.clone(),
            CancellationToken::new(),
            pending.clone(),
        );
        drop(old);
        let registered = pending.lock().unwrap().get(&id).cloned().unwrap();
        assert!(Weak::ptr_eq(&registered, &Arc::downgrade(&current)));
        drop(current);
        assert!(pending.lock().unwrap().is_empty());
    }

    /// A worker whose startup fails must retain the real typed error in its
    /// terminal status, so an observer whose own timeout already fired (no
    /// pending `close()`) can still learn the cause. Regression for #1441.
    #[tokio::test]
    async fn startup_failure_is_retained_in_quit_status() {
        struct StartupFailWorker;

        impl Worker for StartupFailWorker {
            type Error = io::Error;
            type Role = RoleClient;

            fn err_closed() -> Self::Error {
                io::Error::other("worker closed")
            }
            fn err_join(error: tokio::task::JoinError) -> Self::Error {
                io::Error::other(error)
            }
            async fn run(
                self,
                _context: WorkerContext<Self>,
            ) -> Result<(), WorkerQuitReason<Self::Error>> {
                Err(WorkerQuitReason::startup_failed(
                    io::Error::new(io::ErrorKind::ConnectionRefused, "peer unreachable"),
                    "startup request failed",
                ))
            }
        }

        let mut transport = WorkerTransport::spawn(StartupFailWorker);
        // No close() pending — the worker dies on its own; the quit reason must
        // still be observable afterwards.
        transport.close().await.unwrap();
        let status = transport.quit_status().expect("quit reason retained");
        let text = status.to_string();
        assert!(text.contains("Startup failed"), "got: {text}");
        assert!(text.contains("peer unreachable"), "got: {text}");

        // The unified accessor reaches the typed error through the retained
        // wrapper — the same `&E` the live-ack path would expose. This goes
        // through a real `WorkerTransport`, so it cannot regress silently the
        // way a fake transport boxing the bare error would.
        let typed = status
            .downcast_ref::<WorkerTransport<StartupFailWorker>, RoleClient>()
            .expect("downcast_ref reaches the typed error through the retained reason");
        assert_eq!(typed.kind(), io::ErrorKind::ConnectionRefused);

        // The retained reason is downcastable to the typed quit reason, not
        // stringified — source-only data (here the io::Error kind) survives.
        let retained = status
            .error
            .downcast::<RetainedQuitReason<io::Error>>()
            .expect("typed reason downcast");
        match retained.0.as_ref() {
            WorkerQuitReason::StartupFailed { error, .. } => {
                assert_eq!(error.kind(), io::ErrorKind::ConnectionRefused);
            }
            other => panic!("expected StartupFailed, got {other:?}"),
        }
    }
}
