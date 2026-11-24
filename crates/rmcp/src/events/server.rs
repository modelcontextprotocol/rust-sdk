//! Register application event sources and serve the Events catalog and polls.

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
    EXTENSION, LIST, POLL,
    wire::{
        Completion, DEFAULT_POLL_MS, DeliveryMode, EventDescriptor, EventOccurrence, EventsError,
        ListEventsResult, PollParams, PollResult,
    },
};
use crate::{
    RoleServer,
    handler::server::common::{schema_for_output, schema_for_type},
    model::{CustomRequest, CustomResult, JsonObject, ServerCapabilities},
    service::{MaybeSend, MaybeSendFuture, RequestContext, SubscriptionSink},
};

const MAX_SOURCES: usize = 128;
const PAGE_SIZE: usize = 32;
const MAX_BATCH: usize = 64;
const MAX_BATCH_BYTES: usize = 256 * 1024;

/// The MCP request context, including cancellation (`ct`) and middleware extensions.
pub type EventContext = RequestContext<RoleServer>;

macro_rules! hook {
    ($name:ident = $($signature:tt)+) => {
        #[cfg(not(feature = "local"))]
        type $name = dyn $($signature)+ + Send + Sync;
        #[cfg(feature = "local")]
        type $name = dyn $($signature)+;
    };
}
hook!(ValidateArguments = Fn(&JsonObject) -> Result<(), EventsError>);
hook!(NormalizePayload = Fn(JsonObject) -> Result<JsonObject, EventsError>);
hook!(Authorize = Fn(EventContext, JsonObject, DeliveryMode) -> HookFuture<Result<(), EventsError>>);
hook!(Check = Fn(EventContext, JsonObject, CheckQuery) -> HookFuture<Result<CheckedBatch, EventsError>>);
#[cfg(not(feature = "local"))]
type HookFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;
#[cfg(feature = "local")]
type HookFuture<T> = Pin<Box<dyn Future<Output = T>>>;

/// Server registration and policy for a wire [`EventDescriptor`].
#[derive(Clone)]
pub struct Descriptor {
    wire: EventDescriptor,
    validate_arguments: Shared<ValidateArguments>,
    normalize_payload: Shared<NormalizePayload>,
    authorize: Option<Shared<Authorize>>,
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
    /// Generate schemas from `Args`' deserialize and `Payload`'s serialize
    /// contracts. Validate arguments without changing them. Deserialize and
    /// reserialize each payload to preserve its advertised representation.
    pub fn typed<Args, Payload>(
        name: impl Into<String>,
        description: impl Into<String>,
        delivery: impl IntoIterator<Item = DeliveryMode>,
    ) -> Self
    where
        Args: schemars::JsonSchema + DeserializeOwned + 'static,
        Payload: schemars::JsonSchema + DeserializeOwned + Serialize + 'static,
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
            normalize_payload: Shared::new(|value| {
                let typed = serde_json::from_value::<Payload>(Value::Object(value))
                    .map_err(|_| invalid("payload schema validation failed"))?;
                match serde_json::to_value(typed) {
                    Ok(Value::Object(object)) => Ok(object),
                    _ => Err(invalid("payload must serialize to an object")),
                }
            }),
            authorize: None,
        }
    }

    /// Supply wire schemas and separate validators responsible for enforcing
    /// those schemas. Do not annotate sensitive arguments as HTTP headers.
    pub fn raw(
        descriptor: EventDescriptor,
        validate_arguments: impl Fn(&JsonObject) -> Result<(), EventsError> + MaybeSend + 'static,
        validate_payload: impl Fn(&JsonObject) -> Result<(), EventsError> + MaybeSend + 'static,
    ) -> Self {
        Self {
            wire: descriptor,
            validate_arguments: Shared::new(validate_arguments),
            normalize_payload: Shared::new(move |value| {
                validate_payload(&value)?;
                Ok(value)
            }),
            authorize: None,
        }
    }

    /// Check permissions using the existing request context before reading the
    /// source. Return [`EventsError::Forbidden`] to deny access, or another
    /// error for a failure such as an unavailable authorization backend.
    /// Omitting this hook means allow-all; authenticated deployments must
    /// supply it for every event type that needs a permission check.
    pub fn authorize<F, Fut>(mut self, authorize: F) -> Self
    where
        F: Fn(EventContext, JsonObject, DeliveryMode) -> Fut + MaybeSend + 'static,
        Fut: Future<Output = Result<(), EventsError>> + MaybeSendFuture + 'static,
    {
        self.authorize = Some(Shared::new(move |ctx, args, mode| {
            Box::pin(authorize(ctx, args, mode))
        }));
        self
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

/// Bounded read requested from an application-owned replay source.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct CheckQuery {
    /// Position to resume strictly after; `None` means return an empty batch at the current head.
    pub cursor: Option<String>,
    /// Earliest eligible event timestamp, when replay is age-limited.
    pub age_floor: Option<DateTime<Utc>>,
    /// Maximum number of items to return.
    pub max_events: usize,
    /// Maximum sum of compact JSON occurrence sizes, excluding item cursors.
    ///
    /// Keep batches within this budget, or supply safe item cursors so the
    /// SDK can return a partial page when the batch exceeds it.
    pub max_bytes: usize,
}

/// A bounded ordered read with source-owned, durable positions.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct CheckedBatch {
    /// Only on a gap: a fresh replay position before the first eligible item.
    /// The SDK returns an empty truncated page at this position.
    pub start: Option<String>,
    /// Occurrences in order. A cursor on an item must resume strictly after
    /// that item without skipping any later item. Poll strips item cursors
    /// from the wire, but uses them to cut oversized batches safely.
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

/// Opt-in catalog and sources shared by handler instances.
///
/// An omitted authorizer means allow-all. Authenticated deployments MUST
/// supply [`Descriptor::authorize`] for every event type that needs a
/// permission check. The SDK cannot detect application authentication.
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
}

enum Source {
    Check(Shared<Check>),
}

fn invalid(reason: impl Into<String>) -> EventsError {
    EventsError::InvalidParams(reason.into())
}

fn not_found() -> EventsError {
    EventsError::NotFound {
        kind: Some("event".into()),
    }
}

fn limit(name: &str, max: usize) -> EventsError {
    EventsError::ResourceExhausted {
        limit: name.into(),
        max: Some(max as u64),
    }
}

fn unsupported(value: &str) -> EventsError {
    EventsError::Unsupported {
        feature: "deliveryMode".into(),
        value: Some(json!(value)),
        reason: None,
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
        }))
    }

    /// Register a bounded check that owns its cursor format and age seeking.
    /// A null request cursor must return an empty batch at the current head.
    /// Omitting [`Descriptor::authorize`] means allow-all; install it wherever
    /// authenticated permissions need checking.
    pub fn register_check<F, Fut>(
        &self,
        descriptor: Descriptor,
        check: F,
    ) -> Result<(), EventsError>
    where
        F: Fn(EventContext, JsonObject, CheckQuery) -> Fut + MaybeSend + 'static,
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
        validate_descriptor(&descriptor)?;
        let mut state = self.0.state();
        if state.entries.contains_key(&descriptor.wire.name) {
            return Err(invalid("duplicate event name"));
        }
        if state.entries.len() >= MAX_SOURCES {
            return Err(limit("eventTypes", MAX_SOURCES));
        }
        let generation = state.revision + 1;
        state.entries.insert(
            descriptor.wire.name.clone(),
            Entry {
                descriptor: Shared::new(descriptor),
                registration: generation,
                generation,
                input_generation: generation,
                source,
            },
        );
        self.0.changed(&mut state);
        Ok(generation)
    }

    /// Replace an existing descriptor and invalidate catalog pages.
    /// Application-owned check cursors remain under the source's control.
    pub fn replace(&self, descriptor: Descriptor, change: Replacement) -> Result<(), EventsError> {
        validate_descriptor(&descriptor)?;
        let mut state = self.0.state();
        let generation = state.revision + 1;
        let entry = state
            .entries
            .get_mut(&descriptor.wire.name)
            .ok_or_else(not_found)?;
        if change == Replacement::Breaking {
            if entry.descriptor.wire.input_schema != descriptor.wire.input_schema {
                entry.input_generation = generation;
            }
            entry.generation = generation;
        }
        let previous = std::mem::replace(&mut entry.descriptor, Shared::new(descriptor));
        self.0.changed(&mut state);
        drop(state);
        drop(previous);
        Ok(())
    }

    /// Remove an existing event and invalidate catalog pages.
    pub fn remove(&self, name: &str) -> Result<(), EventsError> {
        let mut state = self.0.state();
        let removed = state.entries.remove(name).ok_or_else(not_found)?;
        self.0.changed(&mut state);
        drop(state);
        drop(removed);
        Ok(())
    }

    pub(crate) fn advertise(&self, capabilities: &mut ServerCapabilities) {
        capabilities.extensions.get_or_insert_default().insert(
            EXTENSION.into(),
            JsonObject::from_iter([("listChanged".into(), json!(true))]),
        );
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
        let offset = match params.cursor {
            None => 0,
            Some(cursor) => {
                let prefix = format!("{}:{}:", self.0.epoch, state.revision);
                cursor
                    .strip_prefix(&prefix)
                    .and_then(|offset| offset.parse::<usize>().ok())
                    .filter(|offset| {
                        *offset > 0 && *offset < state.entries.len() && *offset % PAGE_SIZE == 0
                    })
                    .ok_or_else(|| invalid("invalid or stale catalog page cursor"))?
            }
        };
        let events = state
            .entries
            .values()
            .skip(offset)
            .take(PAGE_SIZE)
            .map(|entry| {
                let mut wire = entry.descriptor.wire.clone();
                wire.delivery.retain(|mode| *mode == DeliveryMode::Poll);
                wire
            })
            .collect();
        let next = offset + PAGE_SIZE;
        let next_cursor = (next < state.entries.len())
            .then(|| format!("{}:{}:{next}", self.0.epoch, state.revision));
        Ok(ListEventsResult {
            result_type: Completion::Complete,
            events,
            next_cursor,
            meta: None,
        })
    }

    pub(crate) async fn dispatch(
        &self,
        request: CustomRequest,
        context: EventContext,
    ) -> Result<CustomResult, EventsError> {
        let method = request.method.as_str();
        if method == LIST {
            return encode(self.list(request.params)?);
        }
        if !super::named_method(method) {
            return Err(EventsError::MethodNotFound);
        }
        let raw = request
            .params
            .ok_or_else(|| invalid("missing event parameters"))?;
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
        crate::transport::streamable_http_server::tower::validate_event_routing(
            &context,
            method,
            &raw,
            name,
            registered
                .as_ref()
                .map(|(descriptor, _, _)| descriptor.wire.input_schema.as_ref()),
        )?;
        let (descriptor, registration, generation) = registered.ok_or_else(not_found)?;
        if method != POLL {
            return Err(unsupported(if method == "events/stream" {
                "push"
            } else {
                "webhook"
            }));
        }
        let params = serde_json::from_value(raw).map_err(|_| invalid("invalid poll parameters"))?;
        encode(
            self.poll(params, context, descriptor, registration, generation)
                .await?,
        )
    }

    async fn poll(
        &self,
        params: PollParams,
        context: EventContext,
        descriptor: Shared<Descriptor>,
        registration: u64,
        generation: u64,
    ) -> Result<PollResult, EventsError> {
        let max_events = params
            .max_events
            .unwrap_or(MAX_BATCH as u64)
            .min(MAX_BATCH as u64) as usize;
        if max_events == 0 {
            return Err(invalid("maxEvents must be positive"));
        }
        let floor = params
            .cursor
            .as_ref()
            .and(params.max_age_ms)
            .and_then(|age| i64::try_from(age).ok())
            .and_then(chrono::TimeDelta::try_milliseconds)
            .and_then(|age| Utc::now().checked_sub_signed(age));
        let args = params.arguments.unwrap_or_default();
        (descriptor.validate_arguments)(&args)?;
        if let Some(authorize) = &descriptor.authorize {
            tokio::select! {
                biased;
                _ = context.ct.cancelled() => return Err(source_error("event authorizer cancelled")),
                allowed = authorize(context.clone(), args.clone(), DeliveryMode::Poll) => allowed?,
            }
        }
        let (current, check) = {
            let state = self.0.state();
            let entry = state.entries.get(&params.name).ok_or_else(not_found)?;
            ensure_current(entry, registration, generation)?;
            let Source::Check(check) = &entry.source;
            (entry.descriptor.clone(), check.clone())
        };
        // The request was authorized against its initial policy snapshot. An
        // additive update of its schema does not invalidate that authorization.
        if !Shared::ptr_eq(&current, &descriptor) {
            (current.validate_arguments)(&args)?;
        }
        let query = CheckQuery {
            cursor: params.cursor,
            age_floor: floor,
            max_events,
            max_bytes: MAX_BATCH_BYTES,
        };
        let batch = tokio::select! {
            biased;
            _ = context.ct.cancelled() => return Err(source_error("event check cancelled")),
            batch = check(context.clone(), args, query.clone()) => batch?,
        };
        let current = {
            let state = self.0.state();
            let entry = state.entries.get(&params.name).ok_or_else(not_found)?;
            ensure_current(entry, registration, generation)?;
            entry.descriptor.clone()
        };
        checked_result(batch, &query, &params.name, &current)
    }
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

fn validate_descriptor(descriptor: &Descriptor) -> Result<(), EventsError> {
    if descriptor.wire.name.is_empty() {
        return Err(invalid("empty event name"));
    }
    for (idx, mode) in descriptor.wire.delivery.iter().enumerate() {
        if descriptor.wire.delivery[..idx].contains(mode) {
            return Err(invalid("duplicate delivery mode"));
        }
    }
    if !descriptor.wire.delivery.contains(&DeliveryMode::Poll) {
        return Err(EventsError::Unsupported {
            feature: "deliveryMode".into(),
            value: Some(json!(descriptor.wire.delivery)),
            reason: Some("this source requires poll delivery".into()),
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
    batch: CheckedBatch,
    query: &CheckQuery,
    name: &str,
    descriptor: &Descriptor,
) -> Result<PollResult, EventsError> {
    let invalid_batch = || source_error("event check violated its replay contract");
    let next = batch.next_poll_ms.unwrap_or(DEFAULT_POLL_MS);
    if batch.end.is_empty() || batch.events.len() > query.max_events {
        return Err(invalid_batch());
    }
    let Some(cursor) = query.cursor.as_deref() else {
        if batch.start.is_some() || !batch.events.is_empty() || batch.has_more {
            return Err(invalid_batch());
        }
        return Ok(poll_result(Vec::new(), batch.end, false, false, next));
    };
    let start = batch.start.as_deref().unwrap_or(cursor);
    if batch
        .start
        .as_ref()
        .is_some_and(|start| start.is_empty() || start == cursor)
        || ((batch.has_more || !batch.events.is_empty()) && batch.end == start)
        || (batch.start.is_some() && batch.end == cursor)
    {
        return Err(invalid_batch());
    }
    let mut events = Vec::new();
    let mut bytes = 0;
    let mut cut = None;
    let mut overflow = false;
    for mut event in batch.events {
        if event.event_id.is_empty()
            || event.name != name
            || query.age_floor.is_some_and(|floor| event.timestamp < floor)
        {
            return Err(invalid_batch());
        }
        let position = event.cursor.take();
        if position
            .as_ref()
            .is_some_and(|position| position.is_empty() || position == start)
        {
            return Err(invalid_batch());
        }
        if batch.start.is_some() || overflow {
            continue;
        }
        event.data = (descriptor.normalize_payload)(event.data)?;
        let Some(size) = event_bytes(&event)?.filter(|size| bytes + size <= query.max_bytes) else {
            overflow = true;
            continue;
        };
        bytes += size;
        events.push(event);
        if let Some(position) = position {
            cut = Some((events.len(), position));
        }
    }
    if let Some(start) = batch.start {
        let more = batch.has_more || start != batch.end;
        return Ok(poll_result(Vec::new(), start, true, more, next));
    }
    if overflow {
        let (keep, position) = cut.ok_or_else(|| limit("pollBytes", query.max_bytes))?;
        if position == batch.end {
            return Err(invalid_batch());
        }
        events.truncate(keep);
        return Ok(poll_result(events, position, false, true, next));
    }
    Ok(poll_result(events, batch.end, false, batch.has_more, next))
}

fn poll_result(
    events: Vec<EventOccurrence>,
    cursor: String,
    truncated: bool,
    has_more: bool,
    next_poll_ms: u64,
) -> PollResult {
    PollResult {
        result_type: Completion::Complete,
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

    fn contract() -> Descriptor {
        Descriptor::typed::<JsonObject, JsonObject>("sample", "", [DeliveryMode::Poll])
    }

    fn event(name: &str, id: &str, time: DateTime<Utc>) -> EventOccurrence {
        EventOccurrence::new(id, name, time, JsonObject::new())
    }

    #[test]
    fn rejects_invalid_check_contracts() {
        let now = Utc::now();
        let descriptor = contract();
        let mut query = CheckQuery {
            cursor: None,
            age_floor: None,
            max_events: 1,
            max_bytes: MAX_BATCH_BYTES,
        };
        let empty = CheckedBatch::new("end");
        for batch in [
            CheckedBatch::new(""),
            CheckedBatch {
                start: Some("gap".into()),
                ..empty.clone()
            },
            CheckedBatch {
                has_more: true,
                ..empty.clone()
            },
            CheckedBatch {
                events: vec![event("sample", "id", now)],
                ..empty.clone()
            },
        ] {
            assert!(checked_result(batch, &query, "sample", &descriptor).is_err());
        }
        query.cursor = Some("start".into());
        query.age_floor = Some(now);
        let invalid_position = |position: &str| {
            let mut event = event("sample", "id", now);
            event.cursor = Some(position.into());
            event
        };
        for batch in [
            CheckedBatch {
                start: Some("".into()),
                ..empty.clone()
            },
            CheckedBatch {
                start: Some("start".into()),
                ..empty.clone()
            },
            CheckedBatch {
                start: Some("gap".into()),
                end: "start".into(),
                ..empty.clone()
            },
            CheckedBatch {
                has_more: true,
                ..CheckedBatch::new("start")
            },
            CheckedBatch {
                events: vec![event("sample", "id", now)],
                ..CheckedBatch::new("start")
            },
            CheckedBatch {
                events: vec![event("sample", "id", now), event("sample", "id2", now)],
                ..empty.clone()
            },
            CheckedBatch {
                events: vec![event("other", "id", now)],
                ..empty.clone()
            },
            CheckedBatch {
                events: vec![event("sample", "", now)],
                ..empty.clone()
            },
            CheckedBatch {
                events: vec![event(
                    "sample",
                    "id",
                    now - chrono::TimeDelta::milliseconds(1),
                )],
                ..empty.clone()
            },
            CheckedBatch {
                events: vec![invalid_position("start")],
                ..empty.clone()
            },
            CheckedBatch {
                events: vec![invalid_position("")],
                ..empty.clone()
            },
        ] {
            assert!(checked_result(batch, &query, "sample", &descriptor).is_err());
        }
    }

    #[test]
    fn truncated_check_page_exposes_safe_start_before_events() {
        let query = CheckQuery {
            cursor: Some("old".into()),
            age_floor: None,
            max_events: 1,
            max_bytes: MAX_BATCH_BYTES,
        };
        let mut batch = CheckedBatch::new("end");
        batch.start = Some("gap".into());
        batch.events.push(event("sample", "id", Utc::now()));
        let result = checked_result(batch, &query, "sample", &contract()).unwrap();
        assert!(result.truncated && result.has_more);
        assert!(result.events.is_empty());
        assert_eq!(result.cursor.as_deref(), Some("gap"));
    }

    #[test]
    fn normalize_typed_payloads_and_reject_invalid_annotations_without_http() {
        #[derive(Deserialize, Serialize, schemars::JsonSchema)]
        struct Payload {
            x: f64,
        }
        let descriptor =
            Descriptor::typed::<JsonObject, Payload>("sample", "", [DeliveryMode::Poll]);
        let data = json!({"x":1,"extra":true}).as_object().unwrap().clone();
        let normalized = (descriptor.normalize_payload)(data).unwrap();
        assert!(normalized["x"].as_f64().is_some());
        assert!(!normalized.contains_key("extra"));
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
            |_| Ok(()),
        );
        assert!(matches!(
            validate_descriptor(&descriptor),
            Err(EventsError::InvalidParams(_))
        ));
    }
}
