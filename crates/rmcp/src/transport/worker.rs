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
    model::{CancelledNotification, GetExtensions, JsonRpcMessage, RequestId},
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

type RequestCancellations = Arc<Mutex<HashMap<RequestId, Weak<RequestCancellationRegistration>>>>;

/// The transport and generation that delivered an inbound request. Never serialized.
#[derive(Debug, Clone)]
pub(crate) struct ResponseOrigin {
    transport: Arc<AtomicU64>,
    generation: u64,
}

impl ResponseOrigin {
    /// Capture a transport's current generation before handing off a request.
    pub(crate) fn capture(transport: &Arc<AtomicU64>) -> Self {
        Self {
            transport: transport.clone(),
            generation: transport.load(Ordering::SeqCst),
        }
    }

    /// Return whether the originating transport still uses this generation.
    pub(crate) fn is_current(&self) -> bool {
        self.generation == self.transport.load(Ordering::SeqCst)
    }
}

tokio::task_local! {
    static RESPONSE_ORIGIN: Option<ResponseOrigin>;
}

/// Preserve reply origin during both send construction and future polling.
///
/// Lazy wrappers polled in this future retain the context. Wrappers that spawn
/// tasks must construct the inner send before spawning it. Deferring that call
/// to a detached task, or serializing away request extensions, loses the origin.
pub(crate) fn with_response_origin<F>(
    origin: Option<ResponseOrigin>,
    create_send: impl FnOnce() -> F,
) -> impl Future<Output = F::Output> + Send + 'static
where
    F: Future + Send + 'static,
{
    let send = RESPONSE_ORIGIN.sync_scope(origin.clone(), create_send);
    RESPONSE_ORIGIN.scope(origin, send)
}

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
    #[cfg(feature = "transport-streamable-http-client")]
    response_origin: Option<ResponseOrigin>,
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

    /// Return the local generation associated with this send.
    ///
    /// Ordinary sends capture it when [`Transport::send`] creates its future,
    /// before polling or queue admission. Automatic replies retain the generation
    /// that delivered their inbound request. This value is not sent over the wire;
    /// the worker decides whether a message from an older generation is valid.
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
    /// Return the local generation that new sends without a reply origin capture.
    ///
    /// Workers may use this value to check messages from an earlier connection
    /// or session. Automatic replies retain their inbound request's generation.
    /// The generic transport does not check generations automatically.
    pub fn control_generation(&self) -> u64 {
        self.control_generation.load(Ordering::SeqCst)
    }

    /// Advance the local generation, wrapping at [`u64::MAX`], and return its new value.
    ///
    /// Subsequent sends without a reply origin capture the new value. Automatic
    /// replies keep their inbound request's generation even when sent later.
    /// Advancing does not drain queues, cancel work, or reject older messages;
    /// the worker is responsible for those actions.
    pub fn advance_control_generation(&self) -> u64 {
        self.control_generation
            .fetch_add(1, Ordering::SeqCst)
            .wrapping_add(1)
    }

    /// Check both the captured generation and, for replies, the originating transport.
    #[cfg(feature = "transport-streamable-http-client")]
    pub(crate) fn is_current_control(&self, request: &WorkerSendRequest<W>) -> bool {
        request.control_generation == self.control_generation()
            && request
                .response_origin
                .as_ref()
                .is_none_or(|origin| Arc::ptr_eq(&origin.transport, &self.control_generation))
    }

    /// Queue an inbound message, recording request origin before waiting for capacity.
    pub async fn send_to_handler(
        &mut self,
        mut item: RxJsonRpcMessage<W::Role>,
    ) -> Result<(), WorkerQuitReason<W::Error>> {
        if let JsonRpcMessage::Request(request) = &mut item {
            request
                .request
                .extensions_mut()
                .insert(ResponseOrigin::capture(&self.control_generation));
        }
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
        let response_origin = if matches!(
            &item,
            JsonRpcMessage::Response(_) | JsonRpcMessage::Error(_)
        ) {
            RESPONSE_ORIGIN.try_with(Clone::clone).ok().flatten()
        } else {
            None
        };
        let control_generation = response_origin.as_ref().map_or_else(
            || self.control_generation.load(Ordering::SeqCst),
            |origin| origin.generation,
        );
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
            #[cfg(feature = "transport-streamable-http-client")]
            response_origin,
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
            let _quit_reason = handle.await.map_err(W::err_join)?;
            Ok(())
        } else {
            Ok(())
        }
    }
}

#[cfg(all(test, feature = "client"))]
mod tests {
    use std::io;

    use super::*;
    use crate::{
        model::{ClientJsonRpcMessage, ClientResult, ServerJsonRpcMessage},
        service::RoleClient,
    };

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
    async fn inbound_origin_is_captured_before_queue_admission() {
        let (context_tx, context_rx) = tokio::sync::oneshot::channel();
        let mut transport = WorkerTransport::spawn(TestWorker(context_tx));
        let mut context = context_rx.await.unwrap();
        let generation = context.control_generation.clone();
        let request = || {
            serde_json::from_value::<ServerJsonRpcMessage>(serde_json::json!({
                "jsonrpc": "2.0", "id": 7, "method": "ping",
            }))
            .unwrap()
        };
        for _ in 0..WorkerConfig::default().channel_buffer_capacity {
            context.send_to_handler(request()).await.unwrap();
        }
        let mut pending = Box::pin(context.send_to_handler(request()));
        assert!(futures::poll!(pending.as_mut()).is_pending());
        generation.fetch_add(1, Ordering::SeqCst);
        transport.receive().await.unwrap();
        pending.await.unwrap();
        for _ in 1..WorkerConfig::default().channel_buffer_capacity {
            transport.receive().await.unwrap();
        }
        let message = transport.receive().await.unwrap();
        assert_eq!(
            serde_json::to_value(&message).unwrap(),
            serde_json::to_value(request()).unwrap()
        );
        let JsonRpcMessage::Request(request) = message else {
            panic!("expected request")
        };
        let origin = request
            .request
            .extensions()
            .get::<ResponseOrigin>()
            .unwrap();
        assert!(Arc::ptr_eq(&origin.transport, &generation));
        assert_eq!(origin.generation, 0);
        assert!(!origin.is_current());
        transport.close().await.unwrap();
    }

    #[tokio::test]
    async fn reply_origin_survives_send_creation_and_lazy_polling() {
        for response in [
            ClientJsonRpcMessage::response(ClientResult::empty(()), RequestId::Number(7)),
            ClientJsonRpcMessage::error(
                crate::model::ErrorData::internal_error("handler error", None),
                Some(RequestId::Number(7)),
            ),
        ] {
            let (context_tx, context_rx) = tokio::sync::oneshot::channel();
            let mut transport = WorkerTransport::spawn(TestWorker(context_tx));
            let mut context = context_rx.await.unwrap();
            let origin = ResponseOrigin::capture(&context.control_generation);
            context.advance_control_generation();
            let eager =
                with_response_origin(Some(origin.clone()), || transport.send(response.clone()));
            assert!(RESPONSE_ORIGIN.try_with(Clone::clone).is_err());
            let lazy = with_response_origin(Some(origin), || async move {
                tokio::task::yield_now().await;
                let result = transport.send(response).await;
                (transport, result)
            });
            let eager = tokio::spawn(eager);
            let lazy = tokio::spawn(lazy);
            for _ in 0..2 {
                let request = context.from_handler_rx.recv().await.unwrap();
                assert_eq!(request.control_generation(), 0);
                request.responder.send(Ok(())).unwrap();
            }
            eager.await.unwrap().unwrap();
            let (mut transport, result) = lazy.await.unwrap();
            result.unwrap();
            assert!(RESPONSE_ORIGIN.try_with(Clone::clone).is_err());
            transport.close().await.unwrap();
        }
    }

    #[cfg(feature = "transport-streamable-http-client")]
    #[tokio::test]
    async fn reply_origin_checks_transport_identity_even_when_generations_match() {
        let (context_tx, context_rx) = tokio::sync::oneshot::channel();
        let mut transport = WorkerTransport::spawn(TestWorker(context_tx));
        let mut context = context_rx.await.unwrap();
        for foreign in [false, true] {
            let generation = if foreign {
                Arc::new(AtomicU64::new(context.control_generation()))
            } else {
                context.control_generation.clone()
            };
            let origin = ResponseOrigin::capture(&generation);
            let send = with_response_origin(Some(origin), || {
                transport.send(ClientJsonRpcMessage::response(
                    ClientResult::empty(()),
                    RequestId::Number(7),
                ))
            });
            let send = tokio::spawn(send);
            let request = context.from_handler_rx.recv().await.unwrap();
            assert_eq!(request.control_generation(), context.control_generation());
            assert_eq!(context.is_current_control(&request), !foreign);
            request.responder.send(Ok(())).unwrap();
            send.await.unwrap().unwrap();
        }
        transport.close().await.unwrap();
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
}
