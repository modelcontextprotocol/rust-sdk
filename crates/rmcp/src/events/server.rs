//! Register application event sources and serve the Events catalog, polls, and push streams.

#[cfg(feature = "local")]
use std::rc::Rc as Shared;
#[cfg(not(feature = "local"))]
use std::sync::Arc as Shared;
use std::{
    collections::BTreeMap,
    fmt,
    future::Future,
    io,
    pin::Pin,
    sync::{Mutex, MutexGuard},
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::sync::Notify;
use uuid::Uuid;

use super::{
    EXTENSION, LIST, POLL, STREAM,
    wire::{
        DEFAULT_POLL_MS, DeliveryMode, EventDescriptor, EventOccurrence, EventsError,
        ListEventsResult, PollParams, PollResult, not_found, unsupported,
    },
};
use crate::{
    RoleServer,
    handler::server::common::{schema_for_output, schema_for_type},
    model::{CustomRequest, CustomResult, JsonObject, ResultType, ServerCapabilities},
    service::{MaybeSend, MaybeSendFuture, RequestContext, SubscriptionSink},
};

mod emit;
mod push;
pub use emit::EmitHandle;
use emit::{EmitPage, Ring};
#[cfg(feature = "events-webhook")]
pub use webhook::WebhookEgress;

const MAX_BATCH: usize = 64;
const MAX_BATCH_BYTES: usize = 256 * 1024;

macro_rules! hook {
    ($name:ident = $($signature:tt)+) => {
        #[cfg(not(feature = "local"))]
        type $name = dyn $($signature)+ + Send + Sync;
        #[cfg(feature = "local")]
        type $name = dyn $($signature)+;
    };
}
hook!(ValidateArguments = Fn(&JsonObject) -> Result<(), EventsError>);
hook!(Authorize = Fn(RequestContext<RoleServer>, JsonObject, DeliveryMode) -> HookFuture<Result<(), EventsError>>);
hook!(Matches = Fn(&RequestContext<RoleServer>, &EventOccurrence, &JsonObject) -> bool);
hook!(Check = Fn(RequestContext<RoleServer>, JsonObject, CheckQuery) -> HookFuture<Result<CheckedBatch, EventsError>>);
#[cfg(feature = "events-webhook")]
mod webhook;
#[cfg(not(feature = "local"))]
type HookFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;
#[cfg(feature = "local")]
type HookFuture<T> = Pin<Box<dyn Future<Output = T>>>;

/// Server registration and policy for a wire [`EventDescriptor`].
#[derive(Clone)]
pub struct Descriptor {
    wire: EventDescriptor,
    validate_arguments: Shared<ValidateArguments>,
    authorize: Option<Shared<Authorize>>,
    matches: Option<Shared<Matches>>,
}

impl fmt::Debug for Descriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Descriptor")
            .field("name", &self.wire.name)
            .field("delivery", &self.wire.delivery)
            .finish_non_exhaustive()
    }
}

impl Descriptor {
    /// Generate argument and payload schemas and validate incoming arguments.
    pub fn typed<Args, Payload>(
        name: impl Into<String>,
        description: impl Into<String>,
        delivery: impl IntoIterator<Item = DeliveryMode>,
    ) -> Self
    where
        Args: schemars::JsonSchema + DeserializeOwned + 'static,
        Payload: schemars::JsonSchema + 'static,
    {
        Self {
            wire: EventDescriptor::new(
                name,
                description,
                delivery.into_iter().collect(),
                schema_for_type::<Args>(),
                schema_for_output::<Payload>(),
            ),
            validate_arguments: Shared::new(|value| {
                serde_json::from_value::<Args>(Value::Object(value.clone()))
                    .map(|_| ())
                    .map_err(|_| invalid("argument schema validation failed"))
            }),
            authorize: None,
            matches: None,
        }
    }

    /// Supply wire schemas and an argument validator.
    pub fn raw(
        descriptor: EventDescriptor,
        validate_arguments: impl Fn(&JsonObject) -> Result<(), EventsError> + MaybeSend + 'static,
    ) -> Self {
        Self {
            wire: descriptor,
            validate_arguments: Shared::new(validate_arguments),
            authorize: None,
            matches: None,
        }
    }

    /// Check access before reading a source. Omitting the hook means allow-all.
    pub fn authorize<F, Fut>(mut self, authorize: F) -> Self
    where
        F: Fn(RequestContext<RoleServer>, JsonObject, DeliveryMode) -> Fut + MaybeSend + 'static,
        Fut: Future<Output = Result<(), EventsError>> + MaybeSendFuture + 'static,
    {
        self.authorize = Some(Shared::new(move |ctx, args, mode| {
            Box::pin(authorize(ctx, args, mode))
        }));
        self
    }

    /// Filter occurrences from an emit source without changing their positions.
    /// The hook borrows the request context, occurrence, and arguments.
    pub fn matches(
        mut self,
        matches: impl Fn(&RequestContext<RoleServer>, &EventOccurrence, &JsonObject) -> bool
        + MaybeSend
        + 'static,
    ) -> Self {
        self.matches = Some(Shared::new(matches));
        self
    }

    fn validate_source(&self, source: &Source) -> Result<(), EventsError> {
        if matches!(source, Source::Check(_)) && self.matches.is_some() {
            return Err(invalid("matches requires an emit source"));
        }
        Ok(())
    }
}

/// Classify a replacement; the SDK does not infer schema compatibility.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Replacement {
    /// Existing arguments, cursors, and consumers remain compatible.
    Additive,
    /// Stop in-flight reads using the previous schema.
    Breaking,
}

/// Bounded read requested from a replay source.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct CheckQuery {
    /// Position to resume strictly after; `None` means return an empty batch at the current head.
    pub cursor: Option<String>,
    /// Earliest eligible timestamp during age-limited replay.
    pub age_floor: Option<DateTime<Utc>>,
    /// Maximum number of items to return.
    pub max_events: usize,
    /// Maximum compact JSON bytes in a batch.
    pub max_bytes: usize,
}

/// A bounded ordered read with source-owned, durable positions.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct CheckedBatch {
    /// Only on a gap: a fresh replay position before the first eligible item.
    /// The SDK returns an empty truncated page at this position.
    pub start: Option<String>,
    /// Occurrences in order, with optional replay cursors after each item.
    pub events: Vec<EventOccurrence>,
    /// End of scan, not necessarily the position of the last event.
    pub end: String,
    /// Whether more source work remains after `end`.
    pub has_more: bool,
    /// Suggested delay in milliseconds once the caller drains the backlog.
    pub next_poll_ms: Option<u64>,
}

impl CheckedBatch {
    /// Create an empty, untruncated batch ending at a source-issued cursor.
    pub fn new(end: impl Into<String>) -> Self {
        Self {
            start: None,
            events: Vec::new(),
            end: end.into(),
            has_more: false,
            next_poll_ms: None,
        }
    }
}

/// Catalog and event sources shared by server handlers.
///
/// Active push streams may close without a final result when the service shuts down.
#[derive(Clone)]
pub struct EventsServer(Shared<Registry>);

impl fmt::Debug for EventsServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventsServer").finish_non_exhaustive()
    }
}

struct Registry {
    epoch: Uuid,
    state: Mutex<State>,
    changes: Notify,
    #[cfg(feature = "events-webhook")]
    webhooks: Option<Box<webhook::Dispatcher>>,
}

#[derive(Default)]
struct State {
    revision: u64,
    entries: BTreeMap<String, Entry>,
}

struct Entry {
    descriptor: Shared<Descriptor>,
    registration: u64,
    generation: u64,
    input_generation: u64,
    source: Source,
    emitted: Option<Shared<Notify>>,
}

enum Source {
    Check(Shared<Check>),
    Emit(Ring),
}

enum Read {
    Check(Shared<Check>),
    Emit(EmitPage),
}

fn invalid(reason: impl Into<String>) -> EventsError {
    EventsError::InvalidParams(reason.into())
}

fn limit(name: &str, max: usize) -> EventsError {
    EventsError::ResourceExhausted {
        limit: name.into(),
        max: Some(max as u64),
    }
}

fn schema_changed(input_changed: bool) -> EventsError {
    EventsError::Unsupported {
        feature: if input_changed {
            "inputSchema"
        } else {
            "payloadSchema"
        }
        .into(),
        value: None,
        reason: Some("schema_changed".into()),
    }
}

impl Registry {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("Events registry lock poisoned")
    }

    fn changed(&self, state: &mut State) {
        state.revision += 1;
        self.changes.notify_waiters();
    }
}

impl Default for EventsServer {
    fn default() -> Self {
        Self::new()
    }
}

impl EventsServer {
    /// Create a shared runtime with a mutable catalog and list notifications.
    pub fn new() -> Self {
        Self(Shared::new(Registry {
            epoch: Uuid::new_v4(),
            state: Mutex::new(State::default()),
            changes: Notify::new(),
            #[cfg(feature = "events-webhook")]
            webhooks: None,
        }))
    }

    /// Register a source that supplies its own cursors and honors the query bounds.
    /// Return the current head for a null cursor.
    pub fn register_check<F, Fut>(
        &self,
        descriptor: Descriptor,
        check: F,
    ) -> Result<(), EventsError>
    where
        F: Fn(RequestContext<RoleServer>, JsonObject, CheckQuery) -> Fut + MaybeSend + 'static,
        Fut: Future<Output = Result<CheckedBatch, EventsError>> + MaybeSendFuture + 'static,
    {
        self.insert(
            descriptor,
            Source::Check(Shared::new(move |ctx, args, query| {
                Box::pin(check(ctx, args, query))
            })),
        )?;
        Ok(())
    }

    fn insert(&self, descriptor: Descriptor, source: Source) -> Result<u64, EventsError> {
        validate_enabled_descriptor(&descriptor, self.webhooks_enabled())?;
        descriptor.validate_source(&source)?;
        let mut state = self.0.state();
        if state.entries.contains_key(&descriptor.wire.name) {
            return Err(invalid("duplicate event name"));
        }
        let generation = state.revision + 1;
        let emitted = (matches!(&source, Source::Emit(_)) && self.notifies_emits(&descriptor))
            .then(|| Shared::new(Notify::new()));
        state.entries.insert(
            descriptor.wire.name.clone(),
            Entry {
                descriptor: Shared::new(descriptor),
                registration: generation,
                generation,
                input_generation: generation,
                source,
                emitted,
            },
        );
        self.0.changed(&mut state);
        Ok(generation)
    }

    /// Replace a descriptor. Breaking changes end subscriptions and reset emit history.
    pub fn replace(&self, descriptor: Descriptor, change: Replacement) -> Result<(), EventsError> {
        validate_enabled_descriptor(&descriptor, self.webhooks_enabled())?;
        let mut state = self.0.state();
        let generation = state.revision + 1;
        let entry = state
            .entries
            .get_mut(&descriptor.wire.name)
            .ok_or_else(not_found)?;
        descriptor.validate_source(&entry.source)?;
        if change == Replacement::Breaking {
            if entry.descriptor.wire.input_schema != descriptor.wire.input_schema {
                entry.input_generation = generation;
            }
            entry.generation = generation;
            if let Source::Emit(ring) = &mut entry.source {
                *ring = Ring::default();
            }
        }
        if self.notifies_emits(&descriptor) && matches!(&entry.source, Source::Emit(_)) {
            entry
                .emitted
                .get_or_insert_with(|| Shared::new(Notify::new()));
        }
        let previous = std::mem::replace(&mut entry.descriptor, Shared::new(descriptor));
        self.0.changed(&mut state);
        drop(state);
        drop(previous);
        Ok(())
    }

    /// Remove an event type and terminate its subscriptions.
    pub fn remove(&self, name: &str) -> Result<(), EventsError> {
        let mut state = self.0.state();
        let removed = state.entries.remove(name).ok_or_else(not_found)?;
        self.0.changed(&mut state);
        drop(state);
        drop(removed);
        Ok(())
    }

    pub(crate) fn revision(&self) -> u64 {
        self.0.state().revision
    }

    pub(crate) async fn notify(&self, mut revision: u64, sink: SubscriptionSink) {
        loop {
            // Register before inspecting the revision to avoid missing a wake-up.
            let changed = self.0.changes.notified();
            let next = self.revision();
            if next == revision {
                changed.await;
                continue;
            }
            revision = next;
            if sink.notify_events_list_changed().await.is_err() {
                break;
            }
        }
    }

    fn list(&self, params: Option<Value>) -> Result<ListEventsResult, EventsError> {
        #[derive(Deserialize, Default)]
        struct ListParams {
            cursor: Option<String>,
        }
        let params = params
            .map(serde_json::from_value::<ListParams>)
            .transpose()
            .map_err(|_| invalid("invalid catalog parameters"))?
            .unwrap_or_default();
        let state = self.0.state();
        if params.cursor.is_some() {
            return Err(invalid("invalid catalog cursor"));
        }
        let events = state
            .entries
            .values()
            .map(|entry| {
                let mut wire = entry.descriptor.wire.clone();
                wire.delivery.retain(|mode| match mode {
                    DeliveryMode::Webhook => self.webhooks_enabled(),
                    _ => true,
                });
                wire
            })
            .collect();
        Ok(ListEventsResult {
            result_type: ResultType::COMPLETE,
            events,
            next_cursor: None,
            meta: None,
        })
    }

    pub(crate) async fn dispatch(
        &self,
        request: CustomRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<CustomResult, EventsError> {
        let method = request.method.as_str();
        if method == LIST {
            return encode(self.list(request.params)?);
        }
        if !super::named_method(method) {
            return Err(EventsError::MethodNotFound);
        }
        let ct = context.ct.clone();
        tokio::select! {
            biased;
            () = ct.cancelled() => Err(source_error("Events request cancelled")),
            result = self.dispatch_named(method, request.params, context) => result,
        }
    }

    async fn dispatch_named(
        &self,
        method: &str,
        params: Option<Value>,
        context: RequestContext<RoleServer>,
    ) -> Result<CustomResult, EventsError> {
        let raw = params.ok_or_else(|| invalid("missing event parameters"))?;
        let name = raw
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("missing event name"))?;
        let registered = self.0.state().entries.get(name).map(|entry| {
            (
                entry.descriptor.clone(),
                entry.registration,
                entry.generation,
            )
        });
        #[cfg(all(feature = "transport-streamable-http-server", not(feature = "local")))]
        validate_event_routing(
            &context,
            method,
            &raw,
            name,
            registered
                .as_ref()
                .map(|(descriptor, _, _)| descriptor.wire.input_schema.as_ref()),
        )?;
        #[cfg(feature = "events-webhook")]
        if matches!(
            method,
            super::webhook::SUBSCRIBE | super::webhook::UNSUBSCRIBE
        ) && let Some(dispatcher) = &self.0.webhooks
        {
            let principal = dispatcher.principal(&context)?;
            return self
                .dispatch_webhook(method, raw, context, principal, registered)
                .await;
        }
        let (descriptor, registration, generation) = registered.ok_or_else(not_found)?;
        if method == STREAM {
            let params =
                serde_json::from_value(raw).map_err(|_| invalid("invalid stream parameters"))?;
            return self
                .stream(params, context, descriptor, registration, generation)
                .await;
        }
        if method != POLL {
            return Err(unsupported(DeliveryMode::Webhook));
        }
        if !descriptor.wire.delivery.contains(&DeliveryMode::Poll) {
            return Err(unsupported(DeliveryMode::Poll));
        }
        let params = serde_json::from_value(raw).map_err(|_| invalid("invalid poll parameters"))?;
        encode(self.poll(params, context, descriptor).await?)
    }

    fn webhooks_enabled(&self) -> bool {
        #[cfg(feature = "events-webhook")]
        {
            self.0.webhooks.is_some()
        }
        #[cfg(not(feature = "events-webhook"))]
        {
            false
        }
    }

    fn notifies_emits(&self, descriptor: &Descriptor) -> bool {
        let delivery = &descriptor.wire.delivery;
        delivery.contains(&DeliveryMode::Push)
            || (self.webhooks_enabled() && delivery.contains(&DeliveryMode::Webhook))
    }

    async fn poll(
        &self,
        params: PollParams,
        context: RequestContext<RoleServer>,
        descriptor: Shared<Descriptor>,
    ) -> Result<PollResult, EventsError> {
        let max_events = params
            .max_events
            .unwrap_or(MAX_BATCH as u64)
            .min(MAX_BATCH as u64) as usize;
        if max_events == 0 {
            return Err(invalid("maxEvents must be positive"));
        }
        let floor = age_floor(params.cursor.as_deref(), params.max_age_ms);
        let args = params.arguments.unwrap_or_default();
        (descriptor.validate_arguments)(&args)?;
        if let Some(authorize) = &descriptor.authorize {
            authorize(context.clone(), args.clone(), DeliveryMode::Poll).await?;
        }
        let query = CheckQuery {
            cursor: params.cursor,
            age_floor: floor,
            max_events,
            max_bytes: MAX_BATCH_BYTES,
        };
        let (current, read) = {
            let mut state = self.0.state();
            let entry = state.entries.get_mut(&params.name).ok_or_else(not_found)?;
            let read = match &mut entry.source {
                Source::Check(check) => Read::Check(check.clone()),
                Source::Emit(ring) => Read::Emit(ring.read(
                    self.0.epoch,
                    entry.registration,
                    entry.generation,
                    &query,
                )),
            };
            (entry.descriptor.clone(), read)
        };
        if !Shared::ptr_eq(&current, &descriptor) {
            (current.validate_arguments)(&args)?;
            if let Some(authorize) = &current.authorize {
                authorize(context.clone(), args.clone(), DeliveryMode::Poll).await?;
            }
        }
        let check = match read {
            Read::Check(check) => check,
            Read::Emit(page) => return Ok(page.finish(&context, &args, &current, false)),
        };
        let batch = check(context, args, query.clone()).await?;
        checked_result(batch, &query, false)
    }
}

pub(crate) fn set_advertisement(
    events: Option<&EventsServer>,
    capabilities: &mut ServerCapabilities,
    modern: bool,
) {
    if events.is_none() {
        return;
    }
    if modern {
        capabilities.extensions.get_or_insert_default().insert(
            EXTENSION.into(),
            JsonObject::from_iter([("listChanged".into(), json!(true))]),
        );
    } else if let Some(extensions) = &mut capabilities.extensions {
        extensions.remove(EXTENSION);
        if extensions.is_empty() {
            capabilities.extensions = None;
        }
    }
}

#[cfg(all(feature = "transport-streamable-http-server", not(feature = "local")))]
fn validate_event_routing(
    context: &RequestContext<RoleServer>,
    method: &str,
    params: &Value,
    name: &str,
    schema: Option<&JsonObject>,
) -> Result<(), EventsError> {
    if let Some(parts) = context.extensions.get::<http::request::Parts>() {
        crate::transport::common::mcp_headers::validate_routing_headers(
            &parts.headers,
            method,
            Some(params),
            Some(name),
            schema,
        )
        .map_err(EventsError::HeaderMismatch)?;
    }
    Ok(())
}

fn ensure_current(entry: &Entry, registration: u64, generation: u64) -> Result<(), EventsError> {
    if entry.registration != registration {
        Err(not_found())
    } else if entry.generation != generation {
        Err(schema_changed(entry.input_generation > generation))
    } else {
        Ok(())
    }
}

#[cfg(test)]
fn validate_descriptor(descriptor: &Descriptor) -> Result<(), EventsError> {
    validate_enabled_descriptor(descriptor, false)
}

fn validate_enabled_descriptor(
    descriptor: &Descriptor,
    webhooks_enabled: bool,
) -> Result<(), EventsError> {
    if descriptor.wire.name.is_empty() {
        return Err(invalid("empty event name"));
    }
    if !descriptor.wire.delivery.iter().any(|mode| match mode {
        DeliveryMode::Webhook => webhooks_enabled,
        _ => true,
    }) {
        return Err(EventsError::Unsupported {
            feature: "deliveryMode".into(),
            value: Some(json!(descriptor.wire.delivery)),
            reason: Some("this source requires an enabled delivery mode".into()),
        });
    }
    crate::transport::common::mcp_headers::validate_param_header_annotations(
        &descriptor.wire.input_schema,
    )
    .map_err(EventsError::InvalidParams)
}

fn encode(result: impl Serialize) -> Result<CustomResult, EventsError> {
    serde_json::to_value(result)
        .map(CustomResult)
        .map_err(|_| source_error("could not encode Events result"))
}

fn source_error(message: &'static str) -> EventsError {
    EventsError::Remote(Box::new(crate::ErrorData::internal_error(message, None)))
}

fn age_floor(cursor: Option<&str>, max_age_ms: Option<u64>) -> Option<DateTime<Utc>> {
    cursor
        .and(max_age_ms)
        .and_then(|age| i64::try_from(age).ok())
        .and_then(chrono::TimeDelta::try_milliseconds)
        .and_then(|age| Utc::now().checked_sub_signed(age))
}

// None means the occurrence cannot fit the poll byte budget.
fn event_bytes(event: &EventOccurrence) -> Result<Option<usize>, EventsError> {
    struct Counter {
        bytes: usize,
        overflow: bool,
    }
    impl io::Write for Counter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if buf.len() > MAX_BATCH_BYTES - self.bytes {
                self.overflow = true;
                return Err(io::Error::other("event too large"));
            }
            self.bytes += buf.len();
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter {
        bytes: 0,
        overflow: false,
    };
    match serde_json::to_writer(&mut counter, event) {
        Ok(()) => Ok(Some(counter.bytes)),
        Err(_) if counter.overflow => Ok(None),
        Err(_) => Err(source_error("invalid occurrence")),
    }
}

fn checked_result(
    mut batch: CheckedBatch,
    query: &CheckQuery,
    item_cursors: bool,
) -> Result<PollResult, EventsError> {
    let next = batch.next_poll_ms.unwrap_or(DEFAULT_POLL_MS);
    if query.cursor.is_none() {
        return Ok(poll_result(Vec::new(), batch.end, false, false, next));
    }
    if let Some(start) = batch.start {
        let more = batch.has_more || start != batch.end;
        return Ok(poll_result(Vec::new(), start, true, more, next));
    }
    let mut bytes = 0;
    let mut cut = None;
    for (index, event) in batch.events.iter().enumerate() {
        let size = event_bytes(event)?.unwrap_or(usize::MAX);
        if index == query.max_events || size > query.max_bytes - bytes {
            let (keep, cursor) = cut
                .filter(|(_, cursor)| query.cursor.as_ref() != Some(cursor) && *cursor != batch.end)
                .ok_or_else(|| limit("pollBytes", query.max_bytes))?;
            batch.events.truncate(keep);
            batch.end = cursor;
            batch.has_more = true;
            break;
        }
        bytes += size;
        if let Some(cursor) = &event.cursor {
            cut = Some((index + 1, cursor.clone()));
        }
    }
    if !item_cursors {
        for event in &mut batch.events {
            event.cursor = None;
        }
    }
    Ok(poll_result(
        batch.events,
        batch.end,
        false,
        batch.has_more,
        next,
    ))
}

fn poll_result(
    events: Vec<EventOccurrence>,
    cursor: String,
    truncated: bool,
    has_more: bool,
    next_poll_ms: u64,
) -> PollResult {
    PollResult {
        result_type: ResultType::COMPLETE,
        events,
        cursor: Some(cursor),
        truncated,
        has_more,
        next_poll_ms,
        meta: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncated_check_page_exposes_safe_start_before_events() {
        let mut query = CheckQuery {
            cursor: Some("old".into()),
            age_floor: None,
            max_events: 1,
            max_bytes: MAX_BATCH_BYTES,
        };
        let mut batch = CheckedBatch::new("end");
        batch.start = Some("gap".into());
        batch.events.push(EventOccurrence::new(
            "id",
            "sample",
            Utc::now(),
            JsonObject::new(),
        ));
        let result = checked_result(batch.clone(), &query, false).unwrap();
        assert!(result.truncated && result.has_more);
        assert!(result.events.is_empty());
        assert_eq!(result.cursor.as_deref(), Some("gap"));
        query.cursor = None;
        let result = checked_result(batch, &query, false).unwrap();
        assert!(!result.truncated && !result.has_more && result.events.is_empty());
        assert_eq!(result.cursor.as_deref(), Some("end"));
    }

    #[test]
    fn rejects_invalid_annotations_without_http() {
        let schema = json!({"properties":{"x":{"type":"string","x-mcp-header":"not a token"}}});
        let descriptor = Descriptor::raw(
            EventDescriptor::new(
                "sample",
                "",
                vec![DeliveryMode::Poll],
                schema.as_object().unwrap().clone().into(),
                Default::default(),
            ),
            |_| Ok(()),
        );
        assert!(matches!(
            validate_descriptor(&descriptor),
            Err(EventsError::InvalidParams(_))
        ));
    }
}
