//! # Transport
//! The transport type must implemented [`Transport`] trait, which allow it send message concurrently and receive message sequentially.
//！
//! ## Standard Transport Types
//! There are 2 pairs of standard transport types:
//!
//! | transport         | client                                                    | server                                                |
//! |:-:                |:-:                                                        |:-:                                                    |
//! | std IO            | [`child_process::TokioChildProcess`]                      | [`io::stdio`]                                         |
//! | streamable http   | [`streamable_http_client::StreamableHttpClientTransport`] | `streamable_http_server::StreamableHttpService`     |
//!
//！## Helper Transport Types
//! Thers are several helper transport types that can help you to create transport quickly.
//!
//! ### [Worker Transport](`worker::WorkerTransport`)
//! Which allows you to run a worker and process messages in another tokio task.
//!
//! ### [Async Read/Write Transport](`async_rw::AsyncRwTransport`)
//! You need to enable `transport-async-rw` feature to use this transport.
//!
//! This transport is used to create a transport from a byte stream which implemented [`tokio::io::AsyncRead`] and [`tokio::io::AsyncWrite`].
//!
//! This could be very helpful when you want to create a transport from a byte stream, such as a file or a tcp connection.
//!
//! ### [Sink/Stream Transport](`sink_stream::SinkStreamTransport`)
//! This transport is used to create a transport from a sink and a stream.
//!
//! This could be very helpful when you want to create a transport from a duplex object stream, such as a websocket connection.
//!
//! ## [IntoTransport](`IntoTransport`) trait
//! [`IntoTransport`] is a helper trait that implicitly convert a type into a transport type.
//!
//! ### These types is automatically implemented [`IntoTransport`] trait
//! 1. A type that already implement both [`futures::Sink`] and [`futures::Stream`] trait, or a tuple `(Tx, Rx)`  where `Tx` is [`futures::Sink`] and `Rx` is [`futures::Stream`].
//! 2. A type that implement both [`tokio::io::AsyncRead`] and [`tokio::io::AsyncWrite`] trait. or a tuple `(R, W)` where `R` is [`tokio::io::AsyncRead`] and `W` is [`tokio::io::AsyncWrite`].
//! 3. A type that implement [Worker](`worker::Worker`) trait.
//! 4. A type that implement [`Transport`] trait.
//!
//! ## Examples
//!
//! ```rust
//! # use rmcp::{
//! #     ServiceExt, serve_server,
//! # };
//! #[cfg(feature = "client")]
//! # use rmcp::serve_client;
//!
//! // create transport from tcp stream
//! #[cfg(feature = "client")]
//! async fn client() -> Result<(), Box<dyn std::error::Error>> {
//!     let stream = tokio::net::TcpSocket::new_v4()?
//!         .connect("127.0.0.1:8001".parse()?)
//!         .await?;
//!     let client = ().serve(stream).await?;
//!     let tools = client.peer().list_tools(Default::default()).await?;
//!     println!("{:?}", tools);
//!     Ok(())
//! }
//!
//! // create transport from std io
//! #[cfg(feature = "client")]
//! async fn io()  -> Result<(), Box<dyn std::error::Error>> {
//!     let client = ().serve((tokio::io::stdin(), tokio::io::stdout())).await?;
//!     let tools = client.peer().list_tools(Default::default()).await?;
//!     println!("{:?}", tools);
//!     Ok(())
//! }
//! ```

use std::{borrow::Cow, sync::Arc};

use crate::service::{RxJsonRpcMessage, ServiceRole, TxJsonRpcMessage};

pub mod sink_stream;

#[cfg(feature = "transport-async-rw")]
pub mod async_rw;

#[cfg(feature = "transport-worker")]
pub mod worker;
#[cfg(feature = "transport-worker")]
pub use worker::WorkerTransport;

#[cfg(feature = "transport-child-process")]
pub mod child_process;
#[cfg(feature = "which-command")]
pub use child_process::which_command;
#[cfg(feature = "transport-child-process")]
pub use child_process::{ConfigureCommandExt, TokioChildProcess};

#[cfg(feature = "transport-io")]
pub mod io;
#[cfg(feature = "transport-io")]
pub use io::{StdioReader, StdioWriter, stdio, stdio_pipes};

#[cfg(feature = "auth")]
pub mod auth;
#[cfg(feature = "auth-client-credentials-jwt")]
pub use auth::JwtSigningAlgorithm;
#[cfg(feature = "auth")]
pub use auth::{
    AuthClient, AuthError, AuthorizationManager, AuthorizationRequest, AuthorizationSession,
    AuthorizedHttpClient, ClientCredentialsConfig, CredentialRefreshGuard, CredentialStore,
    EXTENSION_OAUTH_CLIENT_CREDENTIALS, InMemoryCredentialStore, InMemoryStateStore,
    OAuthHttpClient, OAuthHttpClientError, OAuthHttpClientFuture, OAuthHttpRedirectPolicy,
    OAuthHttpRequest, ProtectedResourceMetadataInvalidReason, ScopeUpgradeConfig, StateStore,
    StoredAuthorizationState, StoredCredentials, WWWAuthenticateParams,
};

// #[cfg(feature = "transport-ws")]
// pub mod ws;
#[cfg(feature = "transport-streamable-http-server-session")]
pub mod streamable_http_server;
#[cfg(all(feature = "transport-streamable-http-server", not(feature = "local")))]
pub use streamable_http_server::tower::{StreamableHttpServerConfig, StreamableHttpService};

#[cfg(feature = "transport-streamable-http-client")]
pub mod streamable_http_client;
#[cfg(all(unix, feature = "transport-streamable-http-client-unix-socket"))]
pub use common::unix_socket::UnixSocketHttpClient;
#[cfg(feature = "transport-streamable-http-client")]
pub use streamable_http_client::StreamableHttpClientTransport;

/// Common use codes
pub mod common;

pub trait Transport<R>: Send
where
    R: ServiceRole,
{
    type Error: std::error::Error + Send + Sync + 'static;
    fn name() -> Cow<'static, str> {
        std::any::type_name::<Self>().into()
    }
    /// Send a message to the transport
    ///
    /// Notice that the future returned by this function should be `Send` and `'static`.
    /// It's because the sending message could be executed concurrently.
    ///
    fn send(
        &mut self,
        item: TxJsonRpcMessage<R>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static;

    /// Receive a message from the transport, this operation is sequential.
    fn receive(&mut self) -> impl Future<Output = Option<RxJsonRpcMessage<R>>> + Send;

    /// Close the transport
    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Whether this is a synthetic channel-closed error produced after a
    /// background worker disappeared, rather than a real peer/transport error.
    fn is_closed_error(_error: &Self::Error) -> bool {
        false
    }

    /// Whether this transport retains background-worker terminal status through
    /// [`quit_status`](Self::quit_status).
    ///
    /// The default is `false`. Worker-backed transports override this together
    /// with `quit_status`, allowing initialization error recovery to wait for
    /// status publication without closing unrelated transport implementations.
    ///
    /// This is a separate method rather than `quit_status().is_some()` because a
    /// `None` status is ambiguous: the worker may still be running, or the
    /// transport may have no worker at all. Initialization error recovery reads
    /// this to decide whether awaiting `close()` is worthwhile — closing a
    /// worker-backed transport waits for its terminal-status publication,
    /// whereas closing a transport without a worker (a child process, a custom
    /// transport) could block for seconds or forever with no status ever
    /// appearing. Only worker-backed transports should return `true`.
    fn supports_quit_status(&self) -> bool {
        false
    }

    /// Terminal status of the transport's background worker, if it has quit.
    ///
    /// `None` means either the transport has no background worker or the worker
    /// is still running. Implementations backed by a [`Worker`] retain the quit
    /// reason here so an observer whose own timeout already fired can still
    /// learn why the worker died, instead of only seeing a channel closure.
    ///
    /// The cause is returned as an owned [`DynamicTransportError`] wrapping the
    /// worker's typed error so it can cross the service API boundary without a
    /// generic parameter.
    ///
    /// [`Worker`]: crate::transport::worker::Worker
    fn quit_status(&self) -> Option<DynamicTransportError> {
        None
    }
}

pub trait IntoTransport<R, E, A>: Send + 'static
where
    R: ServiceRole,
    E: std::error::Error + Send + 'static,
{
    fn into_transport(self) -> impl Transport<R, Error = E> + 'static;
}

#[non_exhaustive]
pub enum TransportAdapterIdentity {}
impl<R, T, E> IntoTransport<R, E, TransportAdapterIdentity> for T
where
    T: Transport<R, Error = E> + Send + 'static,
    R: ServiceRole,
    E: std::error::Error + Send + Sync + 'static,
{
    fn into_transport(self) -> impl Transport<R, Error = E> + 'static {
        self
    }
}

/// A transport that can send a single message and then close itself
pub struct OneshotTransport<R>
where
    R: ServiceRole,
{
    message: Option<RxJsonRpcMessage<R>>,
    sender: tokio::sync::mpsc::Sender<TxJsonRpcMessage<R>>,
    termination: Arc<tokio::sync::Semaphore>,
}

impl<R> OneshotTransport<R>
where
    R: ServiceRole,
{
    pub fn new(
        message: RxJsonRpcMessage<R>,
    ) -> (Self, tokio::sync::mpsc::Receiver<TxJsonRpcMessage<R>>) {
        let (sender, receiver) = tokio::sync::mpsc::channel(16);
        (
            Self {
                message: Some(message),
                sender,
                termination: Arc::new(tokio::sync::Semaphore::new(0)),
            },
            receiver,
        )
    }
}

impl<R> Transport<R> for OneshotTransport<R>
where
    R: ServiceRole,
{
    type Error = tokio::sync::mpsc::error::SendError<TxJsonRpcMessage<R>>;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<R>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let sender = self.sender.clone();
        let terminate = matches!(item, TxJsonRpcMessage::<R>::Response(_))
            || matches!(item, TxJsonRpcMessage::<R>::Error(_));
        let termination = self.termination.clone();
        async move {
            sender.send(item).await?;
            if terminate {
                termination.add_permits(1);
            }
            Ok(())
        }
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<R>> {
        if let Some(msg) = self.message.take() {
            return Some(msg);
        }
        let _ = self.termination.acquire().await;
        None
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.message.take();
        std::future::ready(Ok(()))
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Transport [{transport_name}] error: {error}")]
#[non_exhaustive]
pub struct DynamicTransportError {
    pub transport_name: Cow<'static, str>,
    pub transport_type_id: std::any::TypeId,
    #[source]
    pub error: Box<dyn std::error::Error + Send + Sync>,
}

impl DynamicTransportError {
    pub fn new<T: Transport<R> + 'static, R: ServiceRole>(e: T::Error) -> Self {
        Self {
            transport_name: T::name(),
            transport_type_id: std::any::TypeId::of::<T>(),
            error: Box::new(e),
        }
    }

    /// Create a `DynamicTransportError` from raw parts.
    ///
    /// Unlike [`new`](Self::new), this does not require a concrete [`Transport`] type,
    /// making it usable in test fixtures and other contexts where a real transport
    /// implementation is not available.
    pub fn from_parts(
        transport_name: impl Into<Cow<'static, str>>,
        transport_type_id: std::any::TypeId,
        error: Box<dyn std::error::Error + Send + Sync>,
    ) -> Self {
        Self {
            transport_name: transport_name.into(),
            transport_type_id,
            error,
        }
    }

    #[cfg(feature = "client")]
    pub(crate) fn is_authorization_required(&self) -> bool {
        let mut error = Some(self.error.as_ref() as &(dyn std::error::Error + 'static));
        while let Some(current) = error {
            #[cfg(feature = "auth")]
            if matches!(
                current.downcast_ref::<auth::AuthError>(),
                Some(auth::AuthError::AuthorizationRequired)
            ) {
                return true;
            }

            #[cfg(feature = "transport-streamable-http-client")]
            if current.is::<streamable_http_client::AuthRequiredError>() {
                return true;
            }

            error = current.source();
        }
        false
    }

    pub fn downcast<T: Transport<R> + 'static, R: ServiceRole>(self) -> Result<T::Error, Self> {
        if !self.is::<T, R>() {
            Err(self)
        } else {
            Ok(self
                .error
                .downcast::<T::Error>()
                .map(|e| *e)
                .expect("type is checked"))
        }
    }

    /// Borrow the transport's typed error, looking through the wrappers used to
    /// share a startup failure between the live acknowledgement and the
    /// retained quit reason.
    ///
    /// [`downcast`](Self::downcast) consumes `self` and returns an owned
    /// `T::Error`, so it cannot work when the typed error is shared via `Arc`
    /// between the in-flight `send()` and the retained quit reason — no owned
    /// value exists to hand back. This accessor borrows instead, so it reaches
    /// the typed error on **both** delivery paths:
    ///
    /// - live acknowledgement: the boxed error is `T::Error` directly;
    /// - retained quit reason (the acknowledgement was lost, e.g. the caller's
    ///   own timeout fired): the boxed error is a
    ///   [`RetainedQuitReason`](worker::RetainedQuitReason) wrapping it.
    ///
    /// Returns `None` when `T` does not match the transport that produced this
    /// error, or when the quit reason carries no typed error (a normal
    /// shutdown).
    ///
    /// For streamable HTTP the borrowed value may still be the
    /// [`StartupShared`](streamable_http_client::StreamableHttpError::StartupShared)
    /// variant; call [`startup_error`](streamable_http_client::StreamableHttpError::startup_error)
    /// on it to recover the original `HttpStatus` / `AuthRequired` / ... variant.
    /// That call is a no-op for every other variant, so it is safe to apply
    /// unconditionally on either delivery path:
    ///
    /// ```ignore
    /// if let Some(e) = error
    ///     .downcast_ref::<StreamableHttpClientTransport<reqwest::Client>, RoleClient>()
    /// {
    ///     match e.startup_error() {
    ///         StreamableHttpError::HttpStatus(m) => { /* m.status, m.retry_after */ }
    ///         StreamableHttpError::AuthRequired(a) => { /* a.www_authenticate_header */ }
    ///         _ => {}
    ///     }
    /// }
    /// ```
    pub fn downcast_ref<T: Transport<R> + 'static, R: ServiceRole>(&self) -> Option<&T::Error> {
        if self.transport_type_id != std::any::TypeId::of::<T>() {
            return None;
        }
        // Live acknowledgement: the typed error is boxed directly.
        if let Some(error) = self.error.downcast_ref::<T::Error>() {
            return Some(error);
        }
        // Retained quit reason: unwrap the shared reason to the same typed error.
        #[cfg(feature = "transport-worker")]
        if let Some(retained) = self
            .error
            .downcast_ref::<worker::RetainedQuitReason<T::Error>>()
        {
            return retained.startup_error();
        }
        None
    }

    pub fn is<T: Transport<R> + 'static, R: ServiceRole>(&self) -> bool {
        self.error.is::<T::Error>() && self.transport_type_id == std::any::TypeId::of::<T>()
    }
}
