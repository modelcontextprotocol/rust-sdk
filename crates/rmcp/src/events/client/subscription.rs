use std::{
    fmt,
    hash::{BuildHasher, RandomState},
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use futures::{Stream, StreamExt, stream};
use serde::Deserialize;
use tokio::{
    sync::mpsc,
    time::{Instant, sleep_until, timeout_at},
};

use super::*;
use crate::{
    events::{
        ACTIVE, ERROR, EVENT, HEARTBEAT, STREAM, StreamParams, TERMINATED,
        wire::{EventOccurrence, terminal},
    },
    model::{ErrorCode, GetMeta, JsonObject, ServerNotification},
    service::{PeerRequestOptions, RequestHandle},
};

const FRAME_CAPACITY: usize = 64;
const POLL_FLOOR_MS: u64 = 1_000;

/// Mode selection and replay position for [`EventsClient::subscribe`].
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct SubscriptionOptions {
    /// Require a particular delivery mode. By default, prefer push to polling.
    pub mode: Option<DeliveryMode>,
    /// Last consumed Events cursor, or `None` to start at the current head.
    pub cursor: Option<String>,
    /// Limit historical replay by age, in milliseconds.
    pub max_age_ms: Option<u64>,
}

/// An ordered update from one Events subscription.
#[derive(Debug)]
#[non_exhaustive]
pub enum EventUpdate {
    /// A delivered occurrence.
    Occurrence(EventOccurrence),
    /// A safe position to persist, including after quiet progress.
    Checkpoint(String),
    /// Some occurrences may have been skipped. Delivered before the new checkpoint.
    Gap,
    /// Delivery can continue, possibly after reconnecting.
    RecoverableError(EventsError),
    /// Delivery ended.
    Terminated(EventsError),
}

#[cfg(not(feature = "local"))]
type Updates = Pin<Box<dyn Stream<Item = EventUpdate> + Send>>;
#[cfg(feature = "local")]
type Updates = Pin<Box<dyn Stream<Item = EventUpdate>>>;

/// One push or managed-poll subscription. Poll the stream to drive delivery.
///
/// A checkpoint follows the events it covers; only persist it after handling
/// those events. Dropping this handle stops polling or cancels the push request.
pub struct EventSubscription {
    updates: Updates,
}

impl fmt::Debug for EventSubscription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventSubscription").finish_non_exhaustive()
    }
}

impl Stream for EventSubscription {
    type Item = EventUpdate;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.updates.as_mut().poll_next(cx)
    }
}

impl EventsClient {
    /// Select a usable catalog mode and create a subscription to one event type.
    ///
    /// The delivery request starts when the returned stream is polled. Drop
    /// the stream to stop delivery without affecting other subscriptions.
    pub async fn subscribe(
        &self,
        name: impl Into<String>,
        arguments: Option<JsonObject>,
        options: SubscriptionOptions,
    ) -> Result<EventSubscription, EventsError> {
        if options.mode == Some(DeliveryMode::Webhook) {
            return Err(unsupported(DeliveryMode::Webhook));
        }
        self.require_advertisement()?;
        let name = name.into();
        let descriptor = self.descriptor_cached(&name, false).await?;
        let mode = match options.mode {
            Some(mode) if supports(&descriptor, mode) => mode,
            Some(mode) => return Err(unsupported(mode)),
            None if supports(&descriptor, DeliveryMode::Push) => DeliveryMode::Push,
            None if supports(&descriptor, DeliveryMode::Poll) => DeliveryMode::Poll,
            None => {
                return Err(EventsError::Unsupported {
                    feature: "deliveryMode".into(),
                    value: None,
                    reason: None,
                });
            }
        };
        let driver = Driver {
            client: EventsClient {
                peer: self.peer.clone(),
                catalog: self.catalog.clone(),
            },
            name,
            arguments,
            mode,
            cursor: options.cursor,
            max_age_ms: options.max_age_ms,
            delivery: Delivery::default(),
            push: None,
            retry: 0,
            next_at: None,
            done: false,
        };
        Ok(EventSubscription {
            updates: Box::pin(
                stream::unfold(driver, |mut driver| async {
                    let item = driver.next().await?;
                    Some((item, driver))
                })
                .fuse(),
            ),
        })
    }
}

fn supports(descriptor: &CatalogEntry, mode: DeliveryMode) -> bool {
    matches!(mode, DeliveryMode::Push | DeliveryMode::Poll) && descriptor.delivery.contains(&mode)
}

// Keep only the current poll page (or one push frame) outside the existing
// bounded notification channel. Advance over it as the consumer pulls.
#[derive(Default)]
struct Delivery {
    events: std::vec::IntoIter<EventOccurrence>,
    event: Option<EventOccurrence>,
    position: Option<(Option<String>, bool)>,
    gap: bool,
}

impl Delivery {
    fn stage_position(&mut self, position: Option<String>, truncated: bool) {
        self.gap = truncated && position.is_some();
        self.position = Some((position, self.gap));
    }

    fn next(&mut self, cursor: &mut Option<String>) -> Option<EventUpdate> {
        if std::mem::take(&mut self.gap) {
            return Some(EventUpdate::Gap);
        }
        if let Some(event) = self.event.take().or_else(|| self.events.next()) {
            return Some(EventUpdate::Occurrence(event));
        }
        if let Some((position, report_gap_checkpoint)) = self.position.take()
            && (*cursor != position || report_gap_checkpoint)
        {
            *cursor = position;
            return cursor.clone().map(EventUpdate::Checkpoint);
        }
        None
    }
}

struct Driver {
    client: EventsClient,
    name: String,
    arguments: Option<JsonObject>,
    mode: DeliveryMode,
    cursor: Option<String>,
    max_age_ms: Option<u64>,
    delivery: Delivery,
    push: Option<Push>,
    retry: u8,
    next_at: Option<Instant>,
    done: bool,
}

impl Driver {
    async fn next(&mut self) -> Option<EventUpdate> {
        while !self.done {
            if let Some(update) = self.delivery.next(&mut self.cursor) {
                return Some(update);
            }
            if let Some(at) = self.next_at.take() {
                sleep_until(at).await;
            }
            let result = match self.mode {
                DeliveryMode::Push => self.push_update().await,
                DeliveryMode::Poll => self.poll_page().await.map(|()| None),
                _ => unreachable!("mode selected from usable catalog delivery"),
            };
            match result {
                Ok(Some(update)) => return Some(update),
                Ok(None) => {}
                Err(error) if self.can_retry(&error) => {
                    self.push = None;
                    let minimum = if self.mode == DeliveryMode::Poll {
                        POLL_FLOOR_MS
                    } else {
                        0
                    };
                    self.next_at = Some(Instant::now() + retry_delay(self.retry, minimum));
                    self.retry = self.retry.saturating_add(1);
                    return Some(EventUpdate::RecoverableError(error));
                }
                Err(error) => return Some(self.finish(error).await),
            }
        }
        None
    }

    fn can_retry(&self, error: &EventsError) -> bool {
        !self.client.peer.is_transport_closed() && retryable(error)
    }

    async fn finish(&mut self, error: EventsError) -> EventUpdate {
        self.done = true;
        self.push = None;
        if terminal(&error) {
            self.client.invalidate_catalog().await;
        }
        EventUpdate::Terminated(error)
    }

    async fn poll_page(&mut self) -> Result<(), EventsError> {
        let params = PollParams {
            name: self.name.clone(),
            arguments: self.arguments.clone(),
            cursor: self.cursor.clone(),
            max_age_ms: self.max_age_ms,
            max_events: None,
        };
        let result = self.client.poll(params).await?;
        if result.events.iter().any(|event| event.name != self.name) {
            return Err(EventsError::UnexpectedResponse);
        }
        self.max_age_ms = None;
        self.retry = 0;
        let progressed = result.cursor.is_some() && self.cursor != result.cursor;
        if !result.has_more || !progressed {
            let delay = if result.has_more {
                POLL_FLOOR_MS
            } else {
                result.next_poll_ms.max(POLL_FLOOR_MS)
            };
            self.next_at = Some(deadline(delay));
        } else {
            tokio::task::yield_now().await;
        }
        self.delivery.events = result.events.into_iter();
        self.delivery
            .stage_position(result.cursor, result.truncated);
        Ok(())
    }

    async fn open(&self, entry: CatalogEntry) -> Result<(Push, Frame), EventsError> {
        if !supports(&entry, DeliveryMode::Push) {
            return Err(unsupported(DeliveryMode::Push));
        }
        let params = StreamParams {
            name: self.name.clone(),
            arguments: self.arguments.clone(),
            cursor: self.cursor.clone(),
            max_age_ms: self.max_age_ms,
            heartbeat_ms: None,
        };
        let request = route(STREAM, encode_params(&params)?, entry.input_schema);
        let deadline = Instant::now() + REQUEST_BUDGET;
        let (request, frames) = timeout_at(
            deadline,
            self.client.peer.send_subscription_request(
                ClientRequest::CustomRequest(request),
                PeerRequestOptions::no_options(),
                FRAME_CAPACITY,
            ),
        )
        .await
        .map_err(|_| timed_out(REQUEST_BUDGET))?
        .map_err(from_service)?;
        let mut push = Push {
            request,
            frames,
            deadline,
            watchdog: REQUEST_BUDGET,
            active: false,
        };
        // An error before activation may require rebuilding the routing headers.
        loop {
            let frame = push.next().await?;
            if !matches!(frame, Frame::Unknown) {
                return Ok((push, frame));
            }
        }
    }

    async fn push_update(&mut self) -> Result<Option<EventUpdate>, EventsError> {
        let frame = match &mut self.push {
            Some(push) => push.next().await?,
            None => {
                let (push, frame) = self
                    .client
                    .with_route(&self.name, |entry| self.open(entry))
                    .await?;
                self.push = Some(push);
                frame
            }
        };
        let push = self.push.as_mut().expect("opened push request");
        if push.active && !matches!(&frame, Frame::Active(_) | Frame::Unknown) {
            self.retry = 0;
        }
        match frame {
            Frame::Unknown => {}
            Frame::Active(active) => {
                if active.heartbeat_ms == 0 {
                    return Err(EventsError::UnexpectedResponse);
                }
                push.active = true;
                push.watchdog = delay(active.heartbeat_ms.saturating_mul(3));
                push.deadline = Instant::now() + push.watchdog;
                self.delivery
                    .stage_position(active.position.cursor, active.truncated);
            }
            _ if !push.active => return Err(EventsError::UnexpectedResponse),
            Frame::Event(event) => {
                if event.name != self.name {
                    return Err(EventsError::UnexpectedResponse);
                }
                let cursor = event.cursor.clone();
                self.delivery.event = Some(event);
                self.delivery.stage_position(cursor, false);
            }
            Frame::Heartbeat(position) => self.delivery.stage_position(position.cursor, false),
            Frame::Error(error) => return Ok(Some(EventUpdate::RecoverableError(error))),
            Frame::Terminated(error) => {
                // Release the pending request even if the peer never writes
                // the final frame. The notification alone ends the subscription.
                return Ok(Some(self.finish(error).await));
            }
        }
        Ok(None)
    }
}

impl Drop for Push {
    fn drop(&mut self) {
        let request = &mut self.request;
        request.peer.unregister_subscription(&request.id);
        if matches!(
            request.rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ) {
            request
                .peer
                .try_cancel_request(request.id.clone(), Some("Events request cancelled".into()));
        }
    }
}

struct Push {
    request: RequestHandle<RoleClient>,
    frames: mpsc::Receiver<ServerNotification>,
    deadline: Instant,
    watchdog: Duration,
    active: bool,
}

impl Push {
    async fn next(&mut self) -> Result<Frame, EventsError> {
        tokio::select! {
            biased;
            Some(frame) = self.frames.recv() => {
                if frame.get_meta().subscription_id().as_ref() != Some(&self.request.id) {
                    return Err(EventsError::UnexpectedResponse);
                }
                if self.active {
                    self.deadline = Instant::now() + self.watchdog;
                }
                Frame::decode(frame)
            }
            result = &mut self.request.rx => {
                if let Ok(Err(error)) = result {
                    return Err(from_service(error));
                }
                Err(from_service(ServiceError::TransportClosed))
            }
            () = sleep_until(self.deadline) => Err(timed_out(self.watchdog)),
        }
    }
}

#[derive(Deserialize)]
struct Position {
    cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Active {
    #[serde(flatten)]
    position: Position,
    #[serde(default)]
    truncated: bool,
    heartbeat_ms: u64,
}

#[derive(Deserialize)]
struct Problem {
    error: crate::ErrorData,
}

enum Frame {
    Active(Active),
    Event(EventOccurrence),
    Heartbeat(Position),
    Error(EventsError),
    Terminated(EventsError),
    Unknown,
}

impl Frame {
    fn decode(notification: ServerNotification) -> Result<Self, EventsError> {
        let ServerNotification::CustomNotification(notification) = notification else {
            return Err(EventsError::UnexpectedResponse);
        };
        if notification.method.starts_with("notifications/events/")
            && !matches!(
                notification.method.as_str(),
                ACTIVE | EVENT | HEARTBEAT | ERROR | TERMINATED
            )
        {
            return Ok(Self::Unknown);
        }
        let value = notification.params.ok_or(EventsError::UnexpectedResponse)?;
        Ok(match notification.method.as_str() {
            ACTIVE => Self::Active(parse(value)?),
            EVENT => {
                let mut event: EventOccurrence = parse(value)?;
                let metadata = notification
                    .extensions
                    .get::<crate::model::NotificationMetaObject>();
                if let Some(metadata) = metadata {
                    event
                        .meta
                        .get_or_insert_default()
                        .extend(metadata.0.0.clone());
                }
                if let Some(meta) = &mut event.meta {
                    meta.remove("io.modelcontextprotocol/subscriptionId");
                    if meta.is_empty() {
                        event.meta = None;
                    }
                }
                Self::Event(event)
            }
            HEARTBEAT => Self::Heartbeat(parse(value)?),
            ERROR => Self::Error(parse::<Problem>(value)?.error.into()),
            TERMINATED => Self::Terminated(parse::<Problem>(value)?.error.into()),
            _ => return Err(EventsError::UnexpectedResponse),
        })
    }
}

fn parse<T: DeserializeOwned>(value: Value) -> Result<T, EventsError> {
    serde_json::from_value(value).map_err(|_| EventsError::UnexpectedResponse)
}

fn timed_out(timeout: Duration) -> EventsError {
    from_service(ServiceError::Timeout { timeout })
}

fn retryable(error: &EventsError) -> bool {
    match error {
        EventsError::Service(error) => matches!(
            error.as_ref(),
            ServiceError::TransportClosed
                | ServiceError::TransportSend(_)
                | ServiceError::Timeout { .. }
                | ServiceError::SubscriptionLagged { .. }
        ),
        // Streamable HTTP also represents request-scoped EOF using -32603.
        EventsError::Remote(error) => error.code == ErrorCode::INTERNAL_ERROR,
        _ => false,
    }
}

fn retry_delay(attempt: u8, minimum: u64) -> Duration {
    let ceiling = (250u64 << attempt.min(5)).max(minimum);
    let jitter = RandomState::new().hash_one(ceiling) % ceiling;
    Duration::from_millis(ceiling + jitter)
}

fn delay(ms: u64) -> Duration {
    Duration::from_millis(ms.min(u64::from(u32::MAX)))
}

fn deadline(ms: u64) -> Instant {
    Instant::now() + delay(ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "server")]
    mod backpressure {
        use tokio::sync::oneshot;
        use tokio_util::sync::CancellationToken;

        use super::*;
        use crate::{
            ClientLifecycleMode, ClientServiceExt, RoleClient, ServerHandler, ServiceExt,
            events::server::{CheckedBatch, Descriptor, EventsServer},
            model::ClientConfig,
            service::{RunningService, spawn_service_task},
        };

        struct App(EventsServer);

        impl ServerHandler for App {
            fn events(&self) -> Option<&EventsServer> {
                Some(&self.0)
            }
        }

        async fn connect(
            runtime: EventsServer,
        ) -> (
            RunningService<RoleClient, ClientConfig>,
            tokio::task::JoinHandle<()>,
        ) {
            let (server, client) = tokio::io::duplex(4096);
            let server = spawn_service_task(async move {
                App(runtime)
                    .serve(server)
                    .await
                    .unwrap()
                    .waiting()
                    .await
                    .unwrap();
            });
            let client = ClientConfig::default()
                .serve_with_lifecycle(
                    client,
                    ClientLifecycleMode::Discover {
                        preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                    },
                )
                .await
                .unwrap();
            (client, server)
        }

        fn source() -> Descriptor {
            Descriptor::typed::<JsonObject, JsonObject>("source", "", [DeliveryMode::Push])
        }

        #[tokio::test(start_paused = true)]
        async fn activation_deadline_with_full_queue() {
            tokio::task::LocalSet::new().run_until(async {
                let runtime = EventsServer::new();
                runtime.register_check(source(), |_, _, _| async {
                    panic!("queue was saturated before the push request opened");
                }).unwrap();
                let (client, server) = connect(runtime).await;
                let api = EventsClient::new(client.peer().clone());
                let mut stream = api.subscribe("source", None, SubscriptionOptions::default()).await.unwrap();
                let reserved = client.reserve_outbound_capacity().await;
                let start = Instant::now();
                assert!(matches!(stream.next().await,
                    Some(EventUpdate::RecoverableError(EventsError::Service(error)))
                    if matches!(*error, ServiceError::Timeout { timeout } if timeout == REQUEST_BUDGET)));
                assert_eq!(start.elapsed(), REQUEST_BUDGET);
                assert!(!client.has_subscription_channels());
                drop(stream);
                drop(reserved);
                tokio::time::timeout(Duration::from_secs(5), api.list_page(None)).await.unwrap().unwrap();
                client.cancel().await.unwrap();
                server.await.unwrap();
            }).await;
        }

        #[tokio::test(start_paused = true)]
        async fn dropping_active_stream_with_full_queue_cancels_source() {
            tokio::task::LocalSet::new().run_until(async {
                let runtime = EventsServer::new();
                let (entered, live) = oneshot::channel();
                let entered = std::sync::Mutex::new(Some(entered));
                let dropped = CancellationToken::new();
                let producer = dropped.clone();
                runtime.register_check(source(), move |_, _, query| {
                    let entered = (query.cursor.as_deref() == Some("head"))
                        .then(|| entered.lock().unwrap().take().unwrap());
                    let producer = producer.clone();
                    async move {
                        if let Some(entered) = entered {
                            let _guard = producer.drop_guard();
                            entered.send(()).unwrap();
                            std::future::pending().await
                        } else {
                            let mut page = CheckedBatch::new("head");
                            page.has_more = true;
                            Ok(page)
                        }
                    }
                }).unwrap();
                let (client, server) = connect(runtime).await;
                let api = EventsClient::new(client.peer().clone());
                let options = SubscriptionOptions { cursor: Some("seed".into()), ..Default::default() };
                let mut stream = api.subscribe("source", None, options).await.unwrap();
                assert!(matches!(stream.next().await, Some(EventUpdate::Checkpoint(cursor)) if cursor == "head"));
                tokio::time::timeout(Duration::from_secs(5), live).await.unwrap().unwrap();
                let reserved = client.reserve_outbound_capacity().await;
                drop(stream);
                assert!(!client.has_subscription_channels());
                tokio::task::yield_now().await;
                assert!(!dropped.is_cancelled());
                drop(reserved);
                tokio::time::timeout(Duration::from_secs(5), dropped.cancelled()).await.unwrap();
                client.cancel().await.unwrap();
                server.await.unwrap();
            }).await;
        }
    }

    #[test]
    fn jitter_stays_in_exponential_bounds() {
        for minimum in [0, POLL_FLOOR_MS] {
            for attempt in 0..=u8::MAX {
                let base: u128 = (250u64 << attempt.min(5)).max(minimum).into();
                assert!((base..base * 2).contains(&(retry_delay(attempt, minimum).as_millis())));
            }
        }
    }

    #[test]
    fn client_retries_internal_errors_but_not_invalid_or_unknown_rpc_errors() {
        let error = |code| {
            from_service(ServiceError::McpError(crate::ErrorData::new(
                ErrorCode(code),
                "fixture",
                None,
            )))
        };
        assert!(retryable(&error(-32603)));
        for code in [-32023, -32024, -32026, -32602, -32601, -32099] {
            assert!(!retryable(&error(code)), "incorrectly retried {code}");
        }
    }
}
