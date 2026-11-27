use std::{pin::pin, time::Duration};

use tokio::time::{Instant, sleep_until, timeout};

use super::{super::wire::terminal, *};
use crate::{
    events::{ACTIVE, ERROR, EVENT, HEARTBEAT, StreamParams, TERMINATED},
    model::{CustomNotification, GetMeta, ServerNotification, SubscriptionsListenResult},
};

const ACTIVATION_BUDGET: Duration = Duration::from_secs(10);
const MIN_HEARTBEAT_MS: u64 = 1_000;
const MAX_HEARTBEAT_MS: u64 = 30_000;
const AUTHORIZE_INTERVAL: Duration = Duration::from_secs(30);

struct Push<'a> {
    server: &'a EventsServer,
    context: RequestContext<RoleServer>,
    name: String,
    args: JsonObject,
    registration: u64,
    generation: u64,
    query: CheckQuery,
    heartbeat_ms: u64,
    last_frame: Instant,
}

#[derive(Default)]
struct Authorization {
    grant: Option<(Shared<Descriptor>, Instant)>,
    idle_retry_at: Option<Instant>,
}

impl EventsServer {
    pub(super) async fn stream(
        &self,
        params: StreamParams,
        context: RequestContext<RoleServer>,
        descriptor: Shared<Descriptor>,
        registration: u64,
        generation: u64,
    ) -> Result<CustomResult, EventsError> {
        if !descriptor.wire.delivery.contains(&DeliveryMode::Push) {
            return Err(unsupported(DeliveryMode::Push));
        }
        let mut push = Push {
            server: self,
            name: params.name,
            args: params.arguments.unwrap_or_default(),
            registration,
            generation,
            query: CheckQuery {
                age_floor: age_floor(params.cursor.as_deref(), params.max_age_ms),
                cursor: params.cursor,
                max_events: MAX_BATCH,
                max_bytes: MAX_BATCH_BYTES,
            },
            context,
            heartbeat_ms: params
                .heartbeat_ms
                .unwrap_or(MAX_HEARTBEAT_MS)
                .clamp(MIN_HEARTBEAT_MS, MAX_HEARTBEAT_MS),
            last_frame: Instant::now(),
        };
        push.run().await
    }
}

impl Push<'_> {
    async fn run(&mut self) -> Result<CustomResult, EventsError> {
        let (emitted, mut descriptor) = {
            let state = self.server.0.state();
            let entry = state.entries.get(&self.name).ok_or_else(not_found)?;
            ensure_current(entry, self.registration, self.generation)?;
            (entry.emitted.clone(), entry.descriptor.clone())
        };
        // Arm before the head snapshot: append can race any read or write.
        let mut wake = emitted.as_ref().map(|emitted| emitted.notified());
        let mut authorization = Authorization::default();
        let mut page = timeout(ACTIVATION_BUDGET, self.read(true, &mut authorization))
            .await
            .map_err(|_| source_error("event stream activation timed out"))??;
        let active_cursor = if page.truncated || page.events.is_empty() {
            page.cursor.clone()
        } else {
            self.query.cursor.clone()
        };
        self.active(active_cursor, page.truncated).await?;

        loop {
            page.has_more &= page.cursor != self.query.cursor;
            if let Err(error) = self.deliver(&mut page).await {
                if terminal(&error) {
                    return self.finish(error).await;
                }
                return Err(error);
            }
            // A quiet, immediately draining backlog must not starve the clock.
            if Instant::now() >= self.heartbeat_at(self.last_frame) {
                self.send(HEARTBEAT, json!({"cursor": self.query.cursor}))
                    .await?;
            }
            if !page.has_more {
                self.query.age_floor = None;
                let at = emitted
                    .is_none()
                    .then(|| Instant::now() + check_delay(page.next_poll_ms));
                match self
                    .wait(wake.take(), at, &descriptor, &mut authorization)
                    .await
                {
                    Ok(()) => {}
                    Err(error) if terminal(&error) => return self.finish(error).await,
                    Err(error) => return Err(error),
                }
            }
            loop {
                wake = emitted.as_ref().map(|emitted| emitted.notified());
                match self.current() {
                    Ok(current) => descriptor = current,
                    Err(error) => return self.finish(error).await,
                }
                match self.read_live(&mut authorization).await {
                    Ok(next) => {
                        page = next;
                        if page.truncated {
                            self.active(page.cursor.clone(), true).await?;
                        }
                        break;
                    }
                    Err(error) if terminal(&error) => return self.finish(error).await,
                    Err(error) => {
                        self.problem(ERROR, error).await?;
                        let retry = Instant::now() + check_delay(page.next_poll_ms);
                        if let Err(error) = self
                            .wait(None, Some(retry), &descriptor, &mut authorization)
                            .await
                        {
                            return self.finish(error).await;
                        }
                    }
                }
            }
        }
    }

    fn current(&self) -> Result<Shared<Descriptor>, EventsError> {
        let state = self.server.0.state();
        let entry = state.entries.get(&self.name).ok_or_else(not_found)?;
        ensure_current(entry, self.registration, self.generation)?;
        Ok(entry.descriptor.clone())
    }

    async fn authorize(
        &self,
        descriptor: &Shared<Descriptor>,
        authorization: &mut Authorization,
    ) -> Result<(), EventsError> {
        let grant = authorization.grant.as_ref();
        let same = grant.is_some_and(|(previous, _)| Shared::ptr_eq(previous, descriptor));
        if same && grant.is_some_and(|(_, until)| Instant::now() < *until) {
            return Ok(());
        }
        if !same {
            (descriptor.validate_arguments)(&self.args)?;
        }
        if let Some(authorize) = &descriptor.authorize {
            authorize(self.context.clone(), self.args.clone(), DeliveryMode::Push).await?;
        }
        authorization.grant = Some((descriptor.clone(), Instant::now() + AUTHORIZE_INTERVAL));
        authorization.idle_retry_at = None;
        Ok(())
    }

    async fn read(
        &self,
        activating: bool,
        authorization: &mut Authorization,
    ) -> Result<PollResult, EventsError> {
        let registry = &self.server.0;
        loop {
            let mut changed = pin!(registry.changes.notified());
            let descriptor = self.current()?;
            if activating && !descriptor.wire.delivery.contains(&DeliveryMode::Push) {
                return Err(unsupported(DeliveryMode::Push));
            }
            let read = async {
                self.authorize(&descriptor, authorization).await?;
                let source = {
                    let mut state = registry.state();
                    let entry = state.entries.get_mut(&self.name).ok_or_else(not_found)?;
                    ensure_current(entry, self.registration, self.generation)?;
                    match &mut entry.source {
                        Source::Check(check) => Read::Check(check.clone()),
                        Source::Emit(ring) => Read::Emit(ring.read(
                            registry.epoch,
                            entry.registration,
                            entry.generation,
                            &self.query,
                        )),
                    }
                };
                match source {
                    Read::Emit(page) => {
                        Ok(page.finish(&self.context, &self.args, &descriptor, true))
                    }
                    Read::Check(check) => {
                        let batch =
                            check(self.context.clone(), self.args.clone(), self.query.clone())
                                .await?;
                        checked_result(batch, &self.query, true)
                    }
                }
            };
            tokio::pin!(read);
            loop {
                tokio::select! {
                    biased;
                    () = &mut changed => {
                        changed.set(registry.changes.notified());
                        if !Shared::ptr_eq(&self.current()?, &descriptor) {
                            break;
                        }
                    }
                    result = &mut read => {
                        if Shared::ptr_eq(&self.current()?, &descriptor) {
                            return result;
                        }
                        break;
                    }
                }
            }
        }
    }

    async fn read_live(
        &mut self,
        authorization: &mut Authorization,
    ) -> Result<PollResult, EventsError> {
        let (last_frame, result) = self
            .heartbeat_while(self.read(false, authorization))
            .await?;
        self.last_frame = last_frame;
        result
    }

    fn heartbeat_at(&self, last_frame: Instant) -> Instant {
        last_frame + Duration::from_millis(self.heartbeat_ms)
    }

    async fn heartbeat_while<T>(
        &self,
        work: impl Future<Output = T>,
    ) -> Result<(Instant, T), EventsError> {
        let mut last_frame = self.last_frame;
        let mut work = pin!(work);
        loop {
            tokio::select! {
                biased;
                result = &mut work => return Ok((last_frame, result)),
                () = sleep_until(self.heartbeat_at(last_frame)) => {
                    self.write(HEARTBEAT, json!({"cursor": self.query.cursor})).await?;
                    last_frame = Instant::now();
                }
            }
        }
    }

    async fn wait(
        &mut self,
        emitted: Option<tokio::sync::futures::Notified<'_>>,
        wake_at: Option<Instant>,
        read_descriptor: &Shared<Descriptor>,
        authorization: &mut Authorization,
    ) -> Result<(), EventsError> {
        let registry = &self.server.0;
        let mut emitted = pin!(emitted);
        loop {
            let mut changed = pin!(registry.changes.notified());
            let descriptor = self.current()?;
            if !Shared::ptr_eq(read_descriptor, &descriptor) {
                return Ok(());
            }
            let heartbeat_at = self.heartbeat_at(self.last_frame);
            let authorize_at = authorization
                .idle_retry_at
                .or_else(|| authorization.grant.as_ref().map(|(_, until)| *until))
                .unwrap_or_else(Instant::now);
            tokio::select! {
                () = &mut changed => continue,
                () = async {
                    match emitted.as_mut().as_pin_mut() {
                        Some(emit) => emit.await,
                        None => std::future::pending().await,
                    }
                } => return Ok(()),
                () = async {
                    match wake_at {
                        Some(at) => sleep_until(at).await,
                        None => std::future::pending().await,
                    }
                } => return Ok(()),
                () = sleep_until(authorize_at), if descriptor.authorize.is_some() => {
                    let (last_frame, result) = tokio::select! {
                        () = &mut changed => {
                            return Ok(());
                        }
                        result = self.heartbeat_while(self.authorize(&descriptor, authorization)) => result?,
                    };
                    self.last_frame = last_frame;
                    if !Shared::ptr_eq(&self.current()?, read_descriptor) {
                        return Ok(());
                    }
                    match result {
                        Ok(()) => {}
                        Err(error) if terminal(&error) => return Err(error),
                        Err(error) => {
                            self.problem(ERROR, error).await?;
                            authorization.idle_retry_at = Some(Instant::now() + AUTHORIZE_INTERVAL);
                        }
                    }
                }
                () = sleep_until(heartbeat_at) => {
                    self.send(HEARTBEAT, json!({"cursor": self.query.cursor})).await?;
                }
            }
        }
    }

    async fn deliver(&mut self, page: &mut PollResult) -> Result<(), EventsError> {
        let mut events = std::mem::take(&mut page.events).into_iter().peekable();
        while let Some(mut event) = events.next() {
            self.current()?;
            event.cursor = if events.peek().is_none() {
                page.cursor.clone()
            } else {
                event.cursor.or_else(|| self.query.cursor.clone())
            };
            let position = event.cursor.clone();
            self.send(
                EVENT,
                serde_json::to_value(event).map_err(|_| source_error("invalid occurrence"))?,
            )
            .await?;
            self.query.cursor = position;
        }
        self.query.cursor.clone_from(&page.cursor);
        Ok(())
    }

    async fn active(&mut self, cursor: Option<String>, truncated: bool) -> Result<(), EventsError> {
        self.send(
            ACTIVE,
            json!({
                "cursor": cursor, "truncated": truncated, "heartbeatMs": self.heartbeat_ms
            }),
        )
        .await
    }

    async fn finish(&mut self, error: EventsError) -> Result<CustomResult, EventsError> {
        self.problem(TERMINATED, error).await?;
        encode(SubscriptionsListenResult::complete(self.context.id.clone()))
    }

    async fn problem(&mut self, method: &str, error: EventsError) -> Result<(), EventsError> {
        self.send(method, json!({"error": crate::ErrorData::from(error)}))
            .await
    }

    async fn send(&mut self, method: &str, params: Value) -> Result<(), EventsError> {
        self.write(method, params).await?;
        self.last_frame = Instant::now();
        Ok(())
    }

    async fn write(&self, method: &str, mut params: Value) -> Result<(), EventsError> {
        let metadata = params
            .as_object_mut()
            .and_then(|object| object.remove("_meta"));
        let mut notification = CustomNotification::new(method, Some(params));
        if let Some(Value::Object(metadata)) = metadata {
            notification.get_meta_mut().0.0.extend(metadata);
        }
        notification
            .get_meta_mut()
            .set_subscription_id(self.context.id.clone());
        self.context
            .peer
            .send_notification(ServerNotification::CustomNotification(notification))
            .await
            .map_err(|_| source_error("event stream transport closed"))
    }
}

fn check_delay(ms: u64) -> Duration {
    Duration::from_millis(ms.clamp(1_000, 86_400_000))
}
