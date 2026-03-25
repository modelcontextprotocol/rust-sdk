//! Session management for the Streamable HTTP transport.
//!
//! A *session* groups the logically related interactions between a single MCP
//! client and the server, starting from the `initialize` handshake. The server
//! assigns each session a unique [`SessionId`] (returned to the client via the
//! `Mcp-Session-Id` response header) and the client includes that ID on every
//! subsequent request.
//!
//! Two tool calls carrying the same session ID come from the same logical
//! session; different IDs mean different clients or conversations.
//!
//! # Implementations
//!
//! * [`local::LocalSessionManager`] — in-memory session store (default).
//! * [`never::NeverSessionManager`] — rejects all session operations, used
//!   when stateful mode is disabled.
//!
//! # Custom session managers
//!
//! Implement the [`SessionManager`] trait to back sessions with a database,
//! Redis, or any other external store.

use futures::Stream;

pub use crate::transport::common::server_side_http::{ServerSseMessage, SessionId};
use crate::{
    RoleServer,
    model::{ClientJsonRpcMessage, ServerJsonRpcMessage},
};

pub mod local;
pub mod never;
pub mod store;

pub use store::{SessionState, SessionStore, SessionStoreError};

/// Extension marker inserted into the `initialize` request extensions during a
/// session restore replay. Handlers can check for its presence to distinguish a
/// cross-instance restore from a genuine client-initiated `initialize` request.
///
/// ```rust,ignore
/// if req.extensions().get::<SessionRestoreMarker>().is_some() {
///     // this is a restore replay, not a fresh client connection
/// }
/// ```
#[derive(Debug, Clone)]
pub struct SessionRestoreMarker {
    pub id: SessionId,
}

/// The outcome of a [`SessionManager::restore_session`] call.
#[derive(Debug)]
pub enum RestoreOutcome<T> {
    /// The session was just re-created from external state; the caller must
    /// spawn an MCP handler against the returned transport and replay the
    /// `initialize` handshake.
    Restored(T),
    /// The session was already present in memory (e.g. a concurrent request
    /// already restored it). The caller should proceed as if `has_session`
    /// had returned `true` — no further action is required.
    AlreadyPresent,
    /// This session manager does not support external-store restore.
    /// The caller should fall through to the normal 404 response.
    NotSupported,
}

/// Controls how MCP sessions are created, validated, and closed.
///
/// The `StreamableHttpService` calls into this
/// trait for every HTTP request that carries (or should carry) a session ID.
///
/// See the [module-level docs](self) for background on sessions.
pub trait SessionManager: Send + Sync + 'static {
    type Error: std::error::Error + Send + 'static;
    type Transport: crate::transport::Transport<RoleServer>;

    /// Create a new session and return its ID together with the transport
    /// that will be used to exchange MCP messages within this session.
    fn create_session(
        &self,
    ) -> impl Future<Output = Result<(SessionId, Self::Transport), Self::Error>> + Send;

    /// Forward the first message (the `initialize` request) to the session.
    fn initialize_session(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> impl Future<Output = Result<ServerJsonRpcMessage, Self::Error>> + Send;

    /// Return `true` if a session with the given ID exists and is active.
    fn has_session(&self, id: &SessionId)
    -> impl Future<Output = Result<bool, Self::Error>> + Send;

    /// Close and remove the session. Corresponds to an HTTP DELETE request
    /// with `Mcp-Session-Id`.
    fn close_session(&self, id: &SessionId)
    -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Route a client request into the session and return an SSE stream
    /// carrying the server's response(s).
    fn create_stream(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> impl Future<
        Output = Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error>,
    > + Send;

    /// Accept a notification, response, or error message from the client
    /// without producing a response stream.
    fn accept_message(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Create an SSE stream not tied to a specific client request (HTTP GET).
    fn create_standalone_stream(
        &self,
        id: &SessionId,
    ) -> impl Future<
        Output = Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error>,
    > + Send;

    /// Resume an SSE stream from the given `Last-Event-ID`, replaying any
    /// events the client missed.
    fn resume(
        &self,
        id: &SessionId,
        last_event_id: String,
    ) -> impl Future<
        Output = Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error>,
    > + Send;

    /// Attempt to restore a previously-known session from external state,
    /// creating a fresh in-memory session worker with the given `id`.
    ///
    /// See [`RestoreOutcome`] for the three possible results:
    /// - [`RestoreOutcome::Restored`] — session re-created; caller must spawn
    ///   an MCP handler and replay the `initialize` handshake.
    /// - [`RestoreOutcome::AlreadyPresent`] — session is already in memory
    ///   (e.g. a concurrent request restored it first); caller proceeds
    ///   normally.
    /// - [`RestoreOutcome::NotSupported`] (default) — this session manager
    ///   does not support external-store restore; caller returns 404.
    fn restore_session(
        &self,
        _id: SessionId,
    ) -> impl Future<Output = Result<RestoreOutcome<Self::Transport>, Self::Error>> + Send {
        futures::future::ready(Ok(RestoreOutcome::NotSupported))
    }
}
