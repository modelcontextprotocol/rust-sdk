use std::pin::Pin;

use futures::Stream;

use crate::{
    model::InitializeRequestParams, transport::common::server_side_http::ServerSseMessage,
};

/// An opaque identifier for a persisted SSE event.
pub type EventId = String;

/// An opaque identifier for an SSE stream.
pub type StreamId = String;

/// A stream of persisted SSE events in delivery order.
pub type EventStream = Pin<Box<dyn Stream<Item = ServerSseMessage> + Send + Sync + 'static>>;

/// Type alias for boxed event store errors.
pub type EventStoreError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Persistent storage for resumable Streamable HTTP events.
///
/// Implementations typically use a database or distributed log shared by all
/// server instances. Event IDs must be globally unique across all streams, and
/// events must be committed before [`EventStore::store_event`] returns so the
/// returned ID is safe to send to a client.
#[async_trait::async_trait]
pub trait EventStore: Send + Sync + 'static {
    /// Persist an event and return the opaque ID clients should receive.
    ///
    /// The store assigns a globally unique ID and must retain its association
    /// with `stream_id` so a later replay only returns events from that stream.
    async fn store_event(
        &self,
        stream_id: &str,
        event: &ServerSseMessage,
    ) -> Result<EventId, EventStoreError>;

    /// Return events strictly after `last_event_id` in delivery order.
    ///
    /// Implementations must locate the stream from the globally unique event
    /// ID and yield only later events from that stream, with their originally
    /// assigned event IDs.
    ///
    /// A finite stream enables reconnect-and-poll behavior. Implementations
    /// backed by a distributed log may keep the stream open to deliver new
    /// events as they are appended by any server instance.
    async fn replay_events_after(
        &self,
        last_event_id: &str,
    ) -> Result<EventStream, EventStoreError>;
}

impl std::fmt::Debug for dyn EventStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<EventStore>")
    }
}

/// State persisted to an external store for cross-instance session recovery.
///
/// When a client reconnects to a different server instance, the new instance
/// loads this state to transparently replay the `initialize` handshake without
/// the client needing to re-initialize.
#[non_exhaustive]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionState {
    /// Parameters from the client's original `initialize` request.
    pub initialize_params: InitializeRequestParams,
}

impl SessionState {
    pub fn new(initialize_params: InitializeRequestParams) -> Self {
        Self { initialize_params }
    }
}

/// Type alias for boxed session store errors.
pub type SessionStoreError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Pluggable external session store for cross-instance recovery.
///
/// Implement this trait to back sessions with Redis, a database, or any
/// key-value store. The simplest usage is to set
/// `StreamableHttpServerConfig::session_store` to an `Arc<impl SessionStore>`.
///
/// # Example (in-memory, for testing)
///
/// ```rust,ignore
/// use std::{collections::HashMap, sync::Arc};
/// use tokio::sync::RwLock;
/// use rmcp::transport::streamable_http_server::session::store::{
///     SessionState, SessionStore, SessionStoreError,
/// };
///
/// #[derive(Default)]
/// struct InMemoryStore(Arc<RwLock<HashMap<String, SessionState>>>);
///
/// #[async_trait::async_trait]
/// impl SessionStore for InMemoryStore {
///     async fn load(&self, id: &str) -> Result<Option<SessionState>, SessionStoreError> {
///         Ok(self.0.read().await.get(id).cloned())
///     }
///     async fn store(&self, id: &str, state: &SessionState) -> Result<(), SessionStoreError> {
///         self.0.write().await.insert(id.to_owned(), state.clone());
///         Ok(())
///     }
///     async fn delete(&self, id: &str) -> Result<(), SessionStoreError> {
///         self.0.write().await.remove(id);
///         Ok(())
///     }
/// }
/// ```
#[async_trait::async_trait]
pub trait SessionStore: Send + Sync + 'static {
    /// Load session state for the given `session_id`.
    ///
    /// Returns `Ok(None)` when no entry exists (i.e. session is unknown to the store).
    async fn load(&self, session_id: &str) -> Result<Option<SessionState>, SessionStoreError>;

    /// Persist session state for the given `session_id`.
    async fn store(&self, session_id: &str, state: &SessionState) -> Result<(), SessionStoreError>;

    /// Remove session state for the given `session_id`.
    async fn delete(&self, session_id: &str) -> Result<(), SessionStoreError>;
}
