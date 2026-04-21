use crate::model::InitializeRequestParams;

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
