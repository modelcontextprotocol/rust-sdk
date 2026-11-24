#[cfg(feature = "local")]
use std::rc::Weak;
#[cfg(not(feature = "local"))]
use std::sync::Weak;
use std::{collections::VecDeque, fmt, sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use tokio::time::Instant;
use uuid::Uuid;

use super::{
    CheckQuery, DEFAULT_POLL_MS, Descriptor, EventContext, EventOccurrence, EventsError,
    EventsServer, JsonObject, MAX_BATCH_BYTES, PollResult, Registry, Shared, Source,
    ensure_current, event_bytes, limit, not_found, poll_result, schema_changed,
};

const MAX_RECORDS: usize = 256;
const RETENTION: Duration = Duration::from_secs(3600);

/// Producer for a bounded emit source. The producer becomes invalid if the
/// registration is removed, even if the same name is registered again.
#[derive(Clone)]
pub struct EmitHandle {
    registry: Weak<Registry>,
    name: String,
    registration: u64,
}

impl fmt::Debug for EmitHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmitHandle")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl EventsServer {
    /// Register a replay ring limited to 256 events, 256 KiB of encoded
    /// occurrences, and one hour per event type. Older entries are evicted.
    /// Cursors are process-local: use source-affine routing or a durable
    /// check source if polls may reach different replicas.
    ///
    /// Omitting [`Descriptor::authorize`] means allow-all; install it wherever
    /// authenticated permissions need checking.
    pub fn register_emit(&self, descriptor: Descriptor) -> Result<EmitHandle, EventsError> {
        let name = descriptor.wire.name.clone();
        let registration = self.insert(descriptor, Source::Emit(Ring::default()))?;
        Ok(EmitHandle {
            registry: Shared::downgrade(&self.0),
            name,
            registration,
        })
    }
}

impl EmitHandle {
    /// Append an occurrence using a supplied stable upstream ID or a generated
    /// ID when the upstream has none.
    pub fn emit(
        &self,
        event_id: Option<String>,
        timestamp: DateTime<Utc>,
        data: JsonObject,
    ) -> Result<(), EventsError> {
        let registry = self.registry.upgrade().ok_or_else(not_found)?;
        let (mut descriptor, generation) = {
            let state = registry.state();
            let entry = state
                .entries
                .get(&self.name)
                .filter(|entry| entry.registration == self.registration)
                .ok_or_else(not_found)?;
            (entry.descriptor.clone(), entry.generation)
        };
        let id = event_id.unwrap_or_else(|| Uuid::new_v4().to_string());
        if id.is_empty() {
            return Err(super::invalid("empty event ID"));
        }
        let mut event = EventOccurrence::new(id, &self.name, timestamp, data);
        loop {
            event.data = (descriptor.normalize_payload)(event.data)?;
            let bytes = event_bytes(&event)?.ok_or_else(|| limit("eventBytes", MAX_BATCH_BYTES))?;
            let mut state = registry.state();
            let entry = state.entries.get_mut(&self.name).ok_or_else(not_found)?;
            ensure_current(entry, self.registration, generation)?;
            if !Shared::ptr_eq(&entry.descriptor, &descriptor) {
                // The replacement is additive. Revalidate the already-normalized
                // occurrence against the new schema instead of losing it.
                let newer = entry.descriptor.clone();
                drop(state);
                descriptor = newer;
                continue;
            }
            let Source::Emit(ring) = &mut entry.source else {
                return Err(not_found());
            };
            return ring.append(event, bytes);
        }
    }
}

#[derive(Default)]
pub(super) struct Ring {
    records: VecDeque<Record>,
    head: u64,
    bytes: usize,
}

struct Record {
    position: u64,
    inserted: Instant,
    bytes: usize,
    event: Arc<EventOccurrence>,
}

impl Ring {
    fn evict_front(&mut self) {
        if let Some(record) = self.records.pop_front() {
            self.bytes -= record.bytes;
        }
    }

    fn prune(&mut self, now: Instant) {
        while self
            .records
            .front()
            .is_some_and(|record| now.duration_since(record.inserted) >= RETENTION)
        {
            self.evict_front();
        }
    }

    fn append(&mut self, event: EventOccurrence, bytes: usize) -> Result<(), EventsError> {
        let position = self
            .head
            .checked_add(1)
            .ok_or_else(|| limit("emitPositions", usize::MAX))?;
        let inserted = Instant::now();
        self.prune(inserted);
        while self.records.len() >= MAX_RECORDS || self.bytes + bytes > MAX_BATCH_BYTES {
            self.evict_front();
        }
        self.head = position;
        self.bytes += bytes;
        self.records.push_back(Record {
            position,
            inserted,
            bytes,
            event: Arc::new(event),
        });
        Ok(())
    }

    pub(super) fn read(
        &mut self,
        epoch: Uuid,
        registration: u64,
        generation: u64,
        input_generation: u64,
        query: &CheckQuery,
    ) -> Result<EmitPage, EventsError> {
        self.prune(Instant::now());
        let scope = format!("{epoch}:{registration}:{generation}:");
        let boundary = self
            .records
            .front()
            .map_or(self.head, |record| record.position - 1);
        let mut page = EmitPage {
            scope,
            position: self.head,
            records: Vec::new(),
            truncated: false,
            has_more: false,
        };
        let Some(cursor) = query.cursor.as_deref() else {
            return Ok(page);
        };
        let position = cursor
            .strip_prefix(&page.scope)
            .and_then(|pos| pos.parse::<u64>().ok());
        if position.is_none()
            && let Some((prior_generation, prior_position)) = cursor
                .strip_prefix(&format!("{epoch}:{registration}:"))
                .and_then(|rest| rest.split_once(':'))
            && let Ok(prior_generation) = prior_generation.parse::<u64>()
            && prior_generation < generation
            && prior_position.parse::<u64>().is_ok()
        {
            return Err(schema_changed(input_generation > prior_generation));
        }
        page.truncated = position.is_none_or(|pos| pos < boundary || pos > self.head);
        page.position = position
            .filter(|pos| *pos >= boundary && *pos <= self.head)
            .unwrap_or(boundary);
        let old = |record: &Record| {
            query
                .age_floor
                .is_some_and(|floor| record.event.timestamp < floor)
        };
        let start = page.position;
        let mut remaining = self
            .records
            .iter()
            .skip_while(|record| record.position <= start)
            .peekable();
        while remaining.peek().is_some_and(|record| old(record)) {
            if let Some(record) = remaining.next() {
                page.position = record.position;
                page.truncated = true;
            }
        }
        if !page.truncated {
            while let Some(record) = remaining.peek() {
                if page.records.len() == query.max_events || old(record) {
                    break;
                }
                page.records
                    .push((record.position, record.bytes, record.event.clone()));
                remaining.next();
            }
        }
        page.has_more = remaining.peek().is_some();
        Ok(page)
    }
}

pub(super) struct EmitPage {
    scope: String,
    position: u64,
    records: Vec<(u64, usize, Arc<EventOccurrence>)>,
    truncated: bool,
    has_more: bool,
}

impl EmitPage {
    pub(super) fn finish(
        self,
        context: &EventContext,
        args: &JsonObject,
        descriptor: &Descriptor,
    ) -> Result<PollResult, EventsError> {
        let Self {
            scope,
            mut position,
            records,
            truncated,
            mut has_more,
        } = self;
        let mut events = Vec::new();
        let mut bytes = 0;
        for (next, mut size, event) in records {
            if descriptor
                .matches
                .as_ref()
                .is_some_and(|matches| !matches(context, &event, args))
            {
                position = next;
                continue;
            }
            let mut event = (*event).clone();
            if let Some(transform) = &descriptor.transform {
                let transformed = transform(context, &event, args)?;
                event.data = (descriptor.normalize_payload)(transformed)?;
                size = event_bytes(&event)?.ok_or_else(|| limit("eventBytes", MAX_BATCH_BYTES))?;
            }
            if bytes + size > MAX_BATCH_BYTES {
                has_more = true;
                break;
            }
            bytes += size;
            position = next;
            events.push(event);
        }
        Ok(poll_result(
            events,
            format!("{scope}{position}"),
            truncated,
            has_more,
            DEFAULT_POLL_MS,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn empty_age_eviction_and_foreign_or_future_cursor_reset_once() {
        let mut ring = Ring::default();
        let epoch = Uuid::new_v4();
        let mut query = CheckQuery {
            cursor: Some(format!("{epoch}:1:1:0")),
            age_floor: None,
            max_events: 1,
            max_bytes: MAX_BATCH_BYTES,
        };
        let event = EventOccurrence::new("id", "event", Utc::now(), JsonObject::new());
        ring.append(event.clone(), event_bytes(&event).unwrap().unwrap())
            .unwrap();
        tokio::time::advance(RETENTION).await;
        let gap = ring.read(epoch, 1, 1, 1, &query).unwrap();
        assert!(gap.truncated && !gap.has_more && gap.records.is_empty());
        query.cursor = Some(format!("{}{}", gap.scope, gap.position));
        assert!(!ring.read(epoch, 1, 1, 1, &query).unwrap().truncated);
        assert!(
            ring.read(Uuid::new_v4(), 1, 1, 1, &query)
                .unwrap()
                .truncated
        );
        query.cursor = Some(format!("{epoch}:1:1:99999999"));
        assert!(ring.read(epoch, 1, 1, 1, &query).unwrap().truncated);
    }
}
