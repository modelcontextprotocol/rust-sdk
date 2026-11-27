use std::{
    collections::{HashMap, VecDeque},
    time::Duration,
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use bytes::Bytes;
use futures::{StreamExt, stream::FuturesUnordered};
use jiff::Timestamp;
use reqwest::{StatusCode, Url};
use tokio::time::{Instant, sleep_until, timeout_at};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::{
    ErrorData,
    events::webhook::{
        MAX_BODY, SUBSCRIBE, SubscribeParams, UnsubscribeParams, WebhookSecret,
        WebhookSubscription, WebhookSubscriptionStatus, WebhookTtl,
    },
    model::{ErrorCode, Extensions, ResultType},
    service::spawn_service_task,
};

mod egress;
mod key;
pub use egress::WebhookEgress;
use egress::{Egress, Reason, callback_url, echo};
use key::Key;

const RPC_BUDGET: Duration = Duration::from_secs(12);
const DEFAULT_TTL_MS: u64 = 30 * 60 * 1_000;
const MAX_TTL_MS: u64 = 60 * 60 * 1_000;
const ROTATION_GRACE: Duration = Duration::from_secs(60);
const AUTH_INTERVAL: Duration = Duration::from_secs(30);
const PROOF_TTL: Duration = Duration::from_secs(60 * 60);
const VERIFY_INTERVAL: Duration = Duration::from_secs(1);
const MAX_GRANTS: usize = 1024;
const MAX_PROOFS: usize = 4096;
const MAX_PENDING: usize = 4;
const MAX_ATTEMPTS: usize = 5;
const RETRY_WINDOW: Duration = Duration::from_secs(60);

hook!(Principal = Fn(&Extensions) -> Option<String>);

pub(super) struct Dispatcher {
    principal: Shared<Principal>,
    egress: Egress,
    state: Mutex<DispatchState>,
}

#[derive(Default)]
struct DispatchState {
    grants: HashMap<Key, Shared<Grant>>,
    proofs: HashMap<(String, String), Instant>,
    host_limit: HashMap<String, Shared<tokio::sync::Mutex<Instant>>>,
}

struct Grant {
    id: String,
    registration: u64,
    generation: u64,
    stop: CancellationToken,
    changed: Notify,
    policy_changed: Notify,
    data: Mutex<GrantData>,
}

struct GrantData {
    lease: Instant,
    refresh_before: String,
    context: RequestContext<RoleServer>,
    context_revision: u64,
    permitted: Shared<Descriptor>,
    secret: WebhookSecret,
    old_secret: Option<(Instant, WebhookSecret)>,
    cursor: Option<String>,
    truncated: bool,
    last_delivery: Option<String>,
    error: Option<Reason>,
    termination: Option<ErrorData>,
}

type Registration = (Shared<Descriptor>, u64, u64);

fn lock<T>(state: &Mutex<T>) -> MutexGuard<'_, T> {
    state.lock().expect("Events webhook lock poisoned")
}

fn forbidden() -> EventsError {
    EventsError::Forbidden { reason: None }
}

fn missing_grant() -> EventsError {
    EventsError::NotFound {
        kind: Some("subscription".into()),
    }
}

fn callback_error(reason: Reason) -> EventsError {
    EventsError::Remote(Box::new(ErrorData::new(
        ErrorCode(-32027),
        "webhook callback endpoint failed",
        Some(json!({"reason": reason.category()})),
    )))
}

fn random() -> String {
    URL_SAFE_NO_PAD.encode(rand::random::<[u8; 24]>())
}

fn message_id(kind: Option<&str>) -> String {
    let random = random();
    match kind {
        Some(kind) => format!("msg_{kind}_{random}"),
        None => format!("msg_{random}"),
    }
}

fn ttl(hint: WebhookTtl) -> Duration {
    Duration::from_millis(match hint {
        WebhookTtl::Default | WebhookTtl::NoExpiry => DEFAULT_TTL_MS,
        WebhookTtl::Milliseconds(ms) => ms.clamp(1000, MAX_TTL_MS),
    })
}

impl Dispatcher {
    pub(super) fn principal(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> Result<String, EventsError> {
        (self.principal)(&context.extensions)
            .filter(|principal| !principal.is_empty())
            .ok_or_else(forbidden)
    }

    async fn consent(
        &self,
        key: (String, String),
        url: &Url,
        id: &str,
        secret: &WebhookSecret,
        deadline: Instant,
    ) -> Result<(), EventsError> {
        let host = url
            .host_str()
            .expect("validated host")
            .trim_end_matches('.')
            .to_owned();
        let slot = {
            let now = Instant::now();
            let mut state = lock(&self.state);
            state.proofs.retain(|_, expires| *expires > now);
            if state.proofs.contains_key(&key) {
                return Ok(());
            }
            state.host_limit.retain(|_, slot| {
                Shared::strong_count(slot) > 1 || slot.try_lock().is_ok_and(|next| *next > now)
            });
            if let Some(slot) = state.host_limit.get(&host) {
                slot.clone()
            } else {
                if state.host_limit.len() >= MAX_PROOFS {
                    return Err(limit("webhookVerificationHosts", MAX_PROOFS));
                }
                let slot = Shared::new(tokio::sync::Mutex::new(now));
                state.host_limit.insert(host, slot.clone());
                slot
            }
        };
        let exhausted = || EventsError::ResourceExhausted {
            limit: "webhookVerificationRate".into(),
            max: None,
        };
        let mut next = timeout_at(deadline, slot.lock())
            .await
            .map_err(|_| exhausted())?;
        if lock(&self.state)
            .proofs
            .get(&key)
            .is_some_and(|expires| *expires > Instant::now())
        {
            return Ok(());
        }
        if *next >= deadline {
            return Err(exhausted());
        }
        sleep_until(*next).await;
        if Instant::now() >= deadline {
            return Err(exhausted());
        }
        *next = Instant::now() + VERIFY_INTERVAL;
        let nonce = random();
        let body = Bytes::from(json!({"type": "verification", "challenge": nonce}).to_string());
        let response = self
            .egress
            .post(
                url,
                id,
                &message_id(Some("verification")),
                body,
                (secret, None),
            )
            .await
            .map_err(callback_error)?;
        echo(response, &nonce).await.map_err(callback_error)?;
        let mut state = lock(&self.state);
        if state.proofs.len() >= MAX_PROOFS
            && let Some(oldest) = state
                .proofs
                .iter()
                .min_by_key(|(_, expiry)| *expiry)
                .map(|(key, _)| key.clone())
        {
            state.proofs.remove(&oldest);
        }
        state.proofs.insert(key, Instant::now() + PROOF_TTL);
        Ok(())
    }
}

impl DispatchState {
    fn detach(&mut self, key: &Key, grant: &Shared<Grant>) {
        if self
            .grants
            .get(key)
            .is_some_and(|g| Shared::ptr_eq(g, grant))
        {
            self.grants.remove(key);
        }
        grant.stop.cancel();
    }
}

impl GrantData {
    fn live(&self) -> bool {
        self.lease > Instant::now() && self.termination.is_none()
    }

    fn response(&mut self, id: &str, refreshing: bool) -> WebhookSubscription {
        WebhookSubscription {
            result_type: ResultType::COMPLETE,
            id: id.into(),
            refresh_before: Some(self.refresh_before.clone()),
            cursor: self.cursor.clone(),
            truncated: std::mem::take(&mut self.truncated),
            delivery_status: refreshing.then(|| WebhookSubscriptionStatus {
                active: true,
                last_delivery_at: self.last_delivery.clone(),
                last_error: self.error.map(|reason| reason.category().into()),
            }),
        }
    }
}

impl Grant {
    fn refresh(
        &self,
        mut context: RequestContext<RoleServer>,
        secret: &WebhookSecret,
        duration: Duration,
    ) -> Option<WebhookSubscription> {
        let mut data = lock(&self.data);
        if !data.live() || self.stop.is_cancelled() {
            return None;
        }
        context.ct = self.stop.clone();
        data.context = context;
        data.context_revision += 1;
        if !data.secret.same_key(secret) {
            data.old_secret = Some((
                Instant::now() + ROTATION_GRACE,
                std::mem::replace(&mut data.secret, secret.clone()),
            ));
        }
        data.lease = Instant::now() + duration;
        data.refresh_before = crate::time::rfc3339(Timestamp::now() + duration);
        let response = data.response(&self.id, true);
        self.changed.notify_waiters();
        Some(response)
    }

    async fn expiry(&self) {
        loop {
            let changed = self.changed.notified();
            let lease = lock(&self.data).lease;
            tokio::select! {
                biased;
                () = changed => {},
                () = sleep_until(lease) => {
                    if lock(&self.data).lease <= Instant::now() {
                        return;
                    }
                }
            }
        }
    }
}

impl EventsServer {
    /// Configure outbound Events webhooks on a new registry, before sharing
    /// it or creating emit handles.
    ///
    /// `principal` receives only request extensions, not user-supplied
    /// `_meta`. Return a stable, tenant-scoped ID from trusted authentication
    /// middleware, or `None` if the request is unauthenticated. A descriptor's
    /// authorizer remains responsible for the principal's event permissions.
    pub fn with_webhooks(
        mut self,
        principal: impl Fn(&Extensions) -> Option<String> + MaybeSend + 'static,
        egress: WebhookEgress,
    ) -> Result<Self, EventsError> {
        let registry = Shared::get_mut(&mut self.0)
            .ok_or_else(|| invalid("configure webhooks before sharing the Events registry"))?;
        if registry.webhooks.is_some() {
            return Err(invalid("webhooks are already configured"));
        }
        registry.webhooks = Some(Box::new(Dispatcher {
            principal: Shared::new(principal),
            egress: Egress::new(egress)?,
            state: Mutex::default(),
        }));
        Ok(self)
    }

    pub(super) async fn dispatch_webhook(
        &self,
        method: &str,
        raw: Value,
        context: RequestContext<RoleServer>,
        principal: String,
        registration: Option<Registration>,
    ) -> Result<CustomResult, EventsError> {
        let dispatcher = self.0.webhooks.as_ref().expect("checked configuration");
        let deadline = Instant::now() + RPC_BUDGET;
        let work = async {
            if method == SUBSCRIBE {
                let params: SubscribeParams = serde_json::from_value(raw)
                    .map_err(|_| invalid("invalid webhook subscribe parameters"))?;
                let registration = registration.ok_or_else(not_found)?;
                let call = Subscribe {
                    params,
                    context,
                    principal,
                    registration,
                };
                encode(self.subscribe_webhook(dispatcher, call, deadline).await?)
            } else {
                let params: UnsubscribeParams = serde_json::from_value(raw)
                    .map_err(|_| invalid("invalid webhook unsubscribe parameters"))?;
                callback_url(&params.delivery.url)?;
                let args = params.arguments.unwrap_or_default();
                let (descriptor, registration, generation) =
                    registration.ok_or_else(missing_grant)?;
                authorize(&descriptor, &context, &args).await?;
                let key = Key::new(&principal, &params.delivery.url, &params.name, &args)?;
                let registry = self.0.state();
                let entry = registry
                    .entries
                    .get(&params.name)
                    .ok_or_else(missing_grant)?;
                ensure_current(entry, registration, generation)?;
                if !Shared::ptr_eq(&entry.descriptor, &descriptor) {
                    // A descriptor race also needs the client's relist and retry.
                    return Err(EventsError::HeaderMismatch(
                        "event descriptor changed during authorization".into(),
                    ));
                }
                let mut state = lock(&dispatcher.state);
                let grant = state.grants.get(&key).ok_or_else(missing_grant)?;
                if grant.registration != registration
                    || grant.generation != generation
                    || !lock(&grant.data).live()
                    || grant.stop.is_cancelled()
                {
                    return Err(missing_grant());
                }
                grant.stop.cancel();
                state.grants.remove(&key);
                encode(crate::model::TaskAckResult::new())
            }
        };
        timeout_at(deadline, work)
            .await
            .map_err(|_| source_error("webhook RPC budget exceeded"))?
    }

    async fn subscribe_webhook(
        &self,
        dispatcher: &Dispatcher,
        call: Subscribe,
        deadline: Instant,
    ) -> Result<WebhookSubscription, EventsError> {
        let Subscribe {
            params,
            mut context,
            principal,
            registration: (descriptor, registration, generation),
        } = call;
        if !descriptor.wire.delivery.contains(&DeliveryMode::Webhook) {
            return Err(unsupported(DeliveryMode::Webhook));
        }
        let url = callback_url(&params.delivery.url)?;
        let secret = WebhookSecret::from_existing(&params.delivery.secret)?;
        let duration = ttl(params.ttl_ms);
        let args = params.arguments.unwrap_or_default();
        let name = params.name;
        let key = Key::new(&principal, &params.delivery.url, &name, &args)?;
        let id = key.id();
        let source = SourceRef {
            server: self.clone(),
            name,
            args,
            registration,
            generation,
        };
        let grant = {
            let state = lock(&dispatcher.state);
            let grant = state.grants.get(&key);
            if grant.is_none() && state.grants.len() >= MAX_GRANTS {
                return Err(limit("webhookSubscriptions", MAX_GRANTS));
            }
            grant.filter(|grant| source.matches_grant(grant)).cloned()
        };
        let approved = authorize(&descriptor, &context, &source.args).await;
        source.same_descriptor(&descriptor)?;
        if let Err(error) = approved {
            if let EventsError::Forbidden { reason } = &error
                && let Some(grant) = grant
            {
                let mut state = lock(&dispatcher.state);
                lock(&grant.data).termination = Some(
                    EventsError::Forbidden {
                        reason: reason.clone(),
                    }
                    .into(),
                );
                state.detach(&key, &grant);
            }
            return Err(error);
        }
        if let Some(grant) = grant
            && let Some(response) = grant.refresh(context.clone(), &secret, duration)
        {
            return Ok(response);
        }
        dispatcher
            .consent(
                (principal, params.delivery.url),
                &url,
                &id,
                &secret,
                deadline,
            )
            .await?;
        source.same_descriptor(&descriptor)?;
        let mut query = CheckQuery {
            age_floor: age_floor(params.cursor.as_deref(), params.max_age_ms),
            cursor: params.cursor,
            max_events: 1,
            max_bytes: MAX_BODY,
        };
        let mut initial = source.read(&context, &query, &descriptor).await?;
        source.same_descriptor(&descriptor)?;
        let mut state = lock(&dispatcher.state);
        if let Some(grant) = state.grants.get(&key)
            && source.matches_grant(grant)
            && let Some(response) = grant.refresh(context.clone(), &secret, duration)
        {
            return Ok(response);
        }
        if state.grants.len() >= MAX_GRANTS && !state.grants.contains_key(&key) {
            return Err(limit("webhookSubscriptions", MAX_GRANTS));
        }
        let truncated = std::mem::take(&mut initial.truncated);
        if initial.events.is_empty() {
            query.cursor.clone_from(&initial.cursor);
        }
        let stop = CancellationToken::new();
        context.ct = stop.clone();
        let mut data = GrantData {
            lease: Instant::now() + duration,
            refresh_before: crate::time::rfc3339(Timestamp::now() + duration),
            context,
            context_revision: 0,
            permitted: descriptor.clone(),
            secret,
            old_secret: None,
            cursor: query.cursor.clone(),
            truncated,
            last_delivery: None,
            error: None,
            termination: None,
        };
        let response = data.response(&id, false);
        let grant = Shared::new(Grant {
            id,
            registration,
            generation,
            stop,
            changed: Notify::new(),
            policy_changed: Notify::new(),
            data: Mutex::new(data),
        });
        state.grants.insert(key.clone(), grant.clone());
        drop(state);
        let worker = Worker {
            source,
            grant,
            url,
            key,
        };
        spawn_service_task(worker.run(query, initial));
        Ok(response)
    }
}

struct Subscribe {
    params: SubscribeParams,
    context: RequestContext<RoleServer>,
    principal: String,
    registration: Registration,
}

async fn authorize(
    descriptor: &Descriptor,
    context: &RequestContext<RoleServer>,
    args: &JsonObject,
) -> Result<(), EventsError> {
    (descriptor.validate_arguments)(args)?;
    if let Some(authorize) = &descriptor.authorize {
        authorize(context.clone(), args.clone(), DeliveryMode::Webhook).await?;
    }
    Ok(())
}

struct SourceRef {
    server: EventsServer,
    name: String,
    args: JsonObject,
    registration: u64,
    generation: u64,
}

impl SourceRef {
    fn matches_grant(&self, grant: &Grant) -> bool {
        self.registration == grant.registration && self.generation == grant.generation
    }

    fn emitted(&self) -> Option<Shared<Notify>> {
        self.server
            .0
            .state()
            .entries
            .get(&self.name)
            .and_then(|entry| entry.emitted.clone())
    }

    fn current(&self) -> Result<Shared<Descriptor>, EventsError> {
        let state = self.server.0.state();
        let entry = state.entries.get(&self.name).ok_or_else(not_found)?;
        ensure_current(entry, self.registration, self.generation)?;
        if !entry
            .descriptor
            .wire
            .delivery
            .contains(&DeliveryMode::Webhook)
        {
            return Err(unsupported(DeliveryMode::Webhook));
        }
        Ok(entry.descriptor.clone())
    }

    fn same_descriptor(&self, descriptor: &Shared<Descriptor>) -> Result<(), EventsError> {
        if Shared::ptr_eq(&self.current()?, descriptor) {
            Ok(())
        } else {
            // A descriptor race also needs the client's relist and retry.
            Err(EventsError::HeaderMismatch(
                "event descriptor changed during authorization".into(),
            ))
        }
    }

    async fn read(
        &self,
        context: &RequestContext<RoleServer>,
        query: &CheckQuery,
        descriptor: &Shared<Descriptor>,
    ) -> Result<PollResult, EventsError> {
        let registry = &self.server.0;
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
                    query,
                )),
            }
        };
        match source {
            Read::Emit(page) => Ok(page.finish(context, &self.args, descriptor, true)),
            Read::Check(check) => {
                let batch = check(context.clone(), self.args.clone(), query.clone()).await?;
                let (end, more, next) = (batch.end.clone(), batch.has_more, batch.next_poll_ms);
                match checked_result(batch, query, true) {
                    Err(EventsError::ResourceExhausted { limit, .. }) if limit == "pollBytes" => {
                        Ok(poll_result(
                            Vec::new(),
                            end,
                            true,
                            more,
                            next.unwrap_or(DEFAULT_POLL_MS),
                        ))
                    }
                    result => result,
                }
            }
        }
    }
}

struct Worker {
    source: SourceRef,
    grant: Shared<Grant>,
    url: Url,
    key: Key,
}

impl Drop for Worker {
    fn drop(&mut self) {
        lock(&self.dispatcher().state).detach(&self.key, &self.grant);
    }
}

impl Worker {
    fn dispatcher(&self) -> &Dispatcher {
        self.source
            .server
            .0
            .webhooks
            .as_deref()
            .expect("configured dispatcher")
    }

    async fn run(self, query: CheckQuery, initial: PollResult) {
        let error = tokio::select! {
            biased;
            () = self.grant.stop.cancelled() => lock(&self.grant.data).termination.clone(),
            () = self.grant.expiry() => None,
            error = self.policy() => Some(error),
            error = self.deliver(query, initial) => Some(error.into()),
        };
        if let Some(error) = error {
            lock(&self.dispatcher().state).detach(&self.key, &self.grant);
            let body = json!({"type": "terminated", "error": error}).to_string();
            if body.len() <= MAX_BODY {
                let _ = self
                    .post(&message_id(Some("terminated")), body.into())
                    .await;
            }
        }
    }

    async fn policy(&self) -> ErrorData {
        let mut next = Instant::now() + AUTH_INTERVAL;
        'policy: loop {
            let mut changed = std::pin::pin!(self.source.server.0.changes.notified());
            let grant_changed = self.grant.changed.notified();
            let (context, revision, permitted) = {
                let data = lock(&self.grant.data);
                if let Some(error) = &data.termination {
                    return error.clone();
                }
                (
                    data.context.clone(),
                    data.context_revision,
                    data.permitted.clone(),
                )
            };
            let current = match self.source.current() {
                Ok(current) => current,
                Err(error) => return error.into(),
            };
            if !Shared::ptr_eq(&current, &permitted)
                || (current.authorize.is_some() && next <= Instant::now())
            {
                let mut authorization =
                    std::pin::pin!(authorize(&current, &context, &self.source.args));
                tokio::pin!(grant_changed);
                let authorization = loop {
                    tokio::select! {
                        biased;
                        () = &mut changed => {
                            changed.set(self.source.server.0.changes.notified());
                            match self.source.current() {
                                Ok(latest) if Shared::ptr_eq(&latest, &current) => {}
                                Ok(_) => continue 'policy,
                                Err(error) => return error.into(),
                            }
                        }
                        () = &mut grant_changed => continue 'policy,
                        result = &mut authorization => break result,
                    }
                };
                let latest = match self.source.current() {
                    Ok(latest) => latest,
                    Err(error) => return error.into(),
                };
                let mut data = lock(&self.grant.data);
                if revision != data.context_revision || !Shared::ptr_eq(&latest, &current) {
                    continue;
                }
                if let Err(error) = authorization {
                    return error.into();
                }
                data.permitted = current.clone();
                next = Instant::now() + AUTH_INTERVAL;
                self.grant.policy_changed.notify_waiters();
                continue;
            }
            tokio::select! {
                () = changed => {},
                () = grant_changed => {},
                () = sleep_until(next), if current.authorize.is_some() => {},
            }
        }
    }

    async fn post(&self, id: &str, body: Bytes) -> Result<reqwest::Response, Reason> {
        let (secret, previous) = {
            let mut data = lock(&self.grant.data);
            if data
                .old_secret
                .as_ref()
                .is_some_and(|(until, _)| *until <= Instant::now())
            {
                data.old_secret = None;
            }
            (
                data.secret.clone(),
                data.old_secret.as_ref().map(|(_, key)| key.clone()),
            )
        };
        self.dispatcher()
            .egress
            .post(
                &self.url,
                &self.grant.id,
                id,
                body,
                (&secret, previous.as_ref()),
            )
            .await
    }

    async fn attempt(&self, sequence: u64, id: String, body: Bytes) -> u64 {
        let expires = Instant::now() + RETRY_WINDOW;
        for attempt in 0..MAX_ATTEMPTS {
            if Instant::now() >= expires {
                break;
            }
            let response = timeout_at(expires, self.post(&id, body.clone()))
                .await
                .unwrap_or(Err(Reason::Timeout));
            let (settled, failure) = match response {
                Ok(response) => {
                    let status = response.status();
                    if status.is_success() {
                        let mut data = lock(&self.grant.data);
                        data.last_delivery = Some(crate::time::now_rfc3339());
                        data.error = None;
                        return sequence;
                    }
                    (
                        matches!(status, StatusCode::GONE | StatusCode::PAYLOAD_TOO_LARGE),
                        Reason::status(status),
                    )
                }
                Err(reason) => (false, reason),
            };
            lock(&self.grant.data).error = Some(failure);
            if settled || attempt + 1 == MAX_ATTEMPTS {
                break;
            }
            let retry = Instant::now()
                + Duration::from_millis((1000 << attempt) + rand::random_range(0..250_u64));
            if retry >= expires {
                break;
            }
            sleep_until(retry).await;
        }
        sequence
    }

    async fn deliver(&self, mut query: CheckQuery, initial: PollResult) -> EventsError {
        let emitted = self.source.emitted();
        let mut wake = emitted.as_ref().map(|emitted| Box::pin(emitted.notified()));
        let mut attempts = FuturesUnordered::new();
        let mut pending: VecDeque<Pending> = VecDeque::new();
        let mut page = Some(initial);
        let mut sequence = 0_u64;
        let mut read_at = Some(Instant::now());
        let mut activating = true;
        loop {
            let updated = self.grant.policy_changed.notified();
            let changed = self.source.server.0.changes.notified();
            let descriptor = match self.source.current() {
                Ok(current) => current,
                Err(error) => return error,
            };
            let permitted = Shared::ptr_eq(&lock(&self.grant.data).permitted, &descriptor);
            // A gap cannot publish a cursor past an earlier unresolved event.
            let ready = permitted
                && pending.len() < MAX_PENDING
                && !pending.iter().any(|item| item.gap)
                && page
                    .as_ref()
                    .is_none_or(|page| !page.truncated || pending.is_empty());
            if ready && let Some(mut current) = page.take() {
                let cursor = current.cursor.take();
                let mut gap = current.truncated;
                let mut body = None;
                if let Some(mut event) = current.events.pop() {
                    // Beta L521: the message's replay token is the settled
                    // prefix before this attempt, not the occurrence's own position.
                    event.cursor.clone_from(&lock(&self.grant.data).cursor);
                    match event_bytes(&event) {
                        Ok(Some(_)) => match serde_json::to_vec(&event) {
                            Ok(encoded) => body = Some(Bytes::from(encoded)),
                            Err(_) => {
                                return source_error("could not encode webhook occurrence");
                            }
                        },
                        Ok(None) => gap = true,
                        Err(error) => return error,
                    }
                }
                if gap {
                    if !pending.is_empty() {
                        current.cursor = cursor;
                        current.truncated = true;
                        page = Some(current);
                        continue;
                    }
                    let encoded = json!({"type": "gap", "cursor": cursor}).to_string();
                    if encoded.len() > MAX_BODY {
                        return source_error("event cursor exceeds webhook body size");
                    }
                    body = Some(encoded.into());
                }
                current.has_more &= query.cursor != cursor;
                query.cursor.clone_from(&cursor);
                sequence += 1;
                let settled = body.is_none();
                let item = Pending {
                    sequence,
                    cursor,
                    gap,
                    settled,
                };
                pending.push_back(item);
                if let Some(body) = body {
                    attempts.push(self.attempt(sequence, message_id(gap.then_some("gap")), body));
                }
                self.advance(&mut pending);
                if !current.has_more {
                    query.age_floor = None;
                    read_at = if emitted.is_some() {
                        activating.then(Instant::now)
                    } else {
                        Some(
                            Instant::now()
                                + Duration::from_millis(current.next_poll_ms.clamp(1000, 30_000)),
                        )
                    };
                } else {
                    read_at = Some(Instant::now());
                }
                activating = false;
                tokio::task::yield_now().await;
                continue;
            }
            let context = lock(&self.grant.data).context.clone();
            tokio::select! {
                biased;
                () = updated => {},
                () = changed => {},
                Some(acked) = attempts.next(), if !attempts.is_empty() => {
                    if let Some(item) = pending.iter_mut().find(|item| item.sequence == acked) {
                        item.settled = true;
                    }
                    self.advance(&mut pending);
                }
                () = async { wake.as_mut().expect("emit notification").await }, if wake.is_some() => {
                    wake = emitted.as_ref().map(|emitted| Box::pin(emitted.notified()));
                    read_at = Some(Instant::now());
                }
                result = async {
                    sleep_until(read_at.expect("scheduled source read")).await;
                    self.source.read(&context, &query, &descriptor).await
                }, if ready && page.is_none() && read_at.is_some() => {
                    match result {
                        Ok(result) => page = Some(result),
                        Err(error @ (EventsError::Forbidden { .. } | EventsError::NotFound { .. })) => return error,
                        Err(_) => read_at = Some(Instant::now() + Duration::from_secs(1)),
                    }
                }
            }
        }
    }

    fn advance(&self, pending: &mut VecDeque<Pending>) {
        while pending.front().is_some_and(|item| item.settled) {
            let item = pending.pop_front().expect("settled prefix");
            let mut data = lock(&self.grant.data);
            data.cursor = item.cursor;
            data.truncated |= item.gap;
        }
    }
}

struct Pending {
    sequence: u64,
    cursor: Option<String>,
    gap: bool,
    settled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn verification_slot_wait_reports_resource_exhaustion() {
        let server = EventsServer::new()
            .with_webhooks(|_| None, WebhookEgress::default())
            .unwrap();
        let dispatcher = server.0.webhooks.as_ref().unwrap();
        let slot = Shared::new(tokio::sync::Mutex::new(Instant::now()));
        let _in_flight = slot.lock().await;
        lock(&dispatcher.state)
            .host_limit
            .insert("example.com".into(), slot.clone());
        let url = callback_url("https://example.com/callback").unwrap();
        let deadline = Instant::now() + RPC_BUDGET;
        let result = timeout_at(
            deadline,
            dispatcher.consent(
                ("principal".into(), url.to_string()),
                &url,
                "grant",
                &WebhookSecret::generate(),
                deadline,
            ),
        )
        .await
        .unwrap();
        assert!(matches!(result, Err(EventsError::ResourceExhausted { .. })));
    }
}
