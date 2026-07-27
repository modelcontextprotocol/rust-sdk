// Sampling/Roots/Logging are SEP-2577-deprecated; internal references are expected.
#![expect(deprecated)]
pub(super) mod cache;

use std::{borrow::Cow, num::NonZeroUsize, sync::Arc, time::Duration};

use cache::CacheGeneration;
pub use cache::{ClientCacheConfig, MAX_CLIENT_CACHE_TTL};
use thiserror::Error;

use super::*;
use crate::{
    model::{
        ArgumentInfo, CacheScope, CallToolRequest, CallToolRequestParams, CallToolResponse,
        CallToolResult, CancelTaskParams, CancelTaskRequest, CancelledNotification,
        CancelledNotificationParam, ClientInfo, ClientJsonRpcMessage, ClientNotification,
        ClientRequest, ClientResult, CompleteRequest, CompleteRequestParams, CompleteResult,
        CompletionContext, CompletionInfo, DEFAULT_MRTR_MAX_ROUNDS, DiscoverRequest,
        DiscoverRequestParams, DiscoverResult, ErrorData, GetExtensions, GetMeta, GetPromptRequest,
        GetPromptRequestParams, GetPromptResponse, GetPromptResult, GetTaskParams, GetTaskRequest,
        GetTaskResult, InitializeRequest, InitializedNotification, InputRequest,
        InputRequiredResult, InputResponses, JsonRpcResponse, ListPromptsRequest,
        ListPromptsResult, ListResourceTemplatesRequest, ListResourceTemplatesResult,
        ListResourcesRequest, ListResourcesResult, ListToolsRequest, ListToolsResult,
        NumberOrString, PaginatedRequestParams, ProgressNotification, ProgressNotificationParam,
        ProtocolVersion, ReadResourceRequest, ReadResourceRequestParams, ReadResourceResponse,
        ReadResourceResult, Reference, RequestId, RequestMetaObject, RootsListChangedNotification,
        ServerInfo, ServerJsonRpcMessage, ServerNotification, ServerRequest, ServerResult,
        SetLevelRequest, SetLevelRequestParams, SubscribeRequest, SubscribeRequestParams,
        SubscriptionFilter, SubscriptionsListenRequest, SubscriptionsListenRequestParams,
        SubscriptionsListenResult, UnsubscribeRequest, UnsubscribeRequestParams, UpdateTaskParams,
        UpdateTaskRequest,
    },
    transport::DynamicTransportError,
};

/// It represents the error that may occur when serving the client.
///
/// if you want to handle the error, you can use `serve_client_with_ct` or `serve_client` with `Result<RunningService<RoleClient, S>, ClientError>`
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum ClientInitializeError {
    #[error("expect initialized response, but received: {0:?}")]
    ExpectedInitResponse(Option<ServerJsonRpcMessage>),

    #[error("expect initialized result, but received: {0:?}")]
    ExpectedInitResult(Option<ServerResult>),

    #[error("conflict initialized response id: expected {0}, got {1}")]
    ConflictInitResponseId(RequestId, RequestId),

    #[error("connection closed: {0}")]
    ConnectionClosed(String),

    #[error("Send message error {error}, when {context}")]
    TransportError {
        error: DynamicTransportError,
        context: Cow<'static, str>,
    },

    #[error("JSON-RPC error: {0}")]
    JsonRpcError(ErrorData),

    #[error(
        "no compatible protocol version (client: {client_supported:?}, server: {server_supported:?})"
    )]
    NoCompatibleProtocolVersion {
        client_supported: Vec<ProtocolVersion>,
        server_supported: Vec<ProtocolVersion>,
    },

    #[error("discover startup requires at least one preferred protocol version")]
    NoPreferredProtocolVersion,

    #[error("Cancelled")]
    Cancelled,
}

impl ClientInitializeError {
    pub fn transport<T: Transport<RoleClient> + 'static>(
        error: T::Error,
        context: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self::TransportError {
            error: DynamicTransportError::new::<T, _>(error),
            context: context.into(),
        }
    }
}

/// Helper function to get the next message from the stream
async fn expect_next_message<T>(
    transport: &mut T,
    context: &str,
) -> Result<ServerJsonRpcMessage, ClientInitializeError>
where
    T: Transport<RoleClient>,
{
    transport
        .receive()
        .await
        .ok_or_else(|| ClientInitializeError::ConnectionClosed(context.to_string()))
}

/// Helper function to expect a response from the stream
async fn expect_response<T, S>(
    transport: &mut T,
    context: &str,
    service: &S,
    peer: Peer<RoleClient>,
) -> Result<(ServerResult, RequestId), ClientInitializeError>
where
    T: Transport<RoleClient>,
    S: Service<RoleClient>,
{
    loop {
        let message = expect_next_message(transport, context).await?;
        match message {
            // Expected message to complete the initialization
            ServerJsonRpcMessage::Response(JsonRpcResponse { id, result, .. }) => {
                break Ok((result, id));
            }
            // Handle JSON-RPC error responses
            ServerJsonRpcMessage::Error(error) => {
                break Err(ClientInitializeError::JsonRpcError(error.error));
            }
            // Server could send logging messages before handshake
            ServerJsonRpcMessage::Notification(mut notification) => {
                let ServerNotification::LoggingMessageNotification(logging) =
                    &mut notification.notification
                else {
                    tracing::warn!(?notification, "Received unexpected message");
                    continue;
                };

                let mut context = NotificationContext {
                    peer: peer.clone(),
                    meta: NotificationMetaObject::default(),
                    extensions: Extensions::default(),
                };

                if let Some(meta) = logging.extensions.get_mut::<NotificationMetaObject>() {
                    std::mem::swap(&mut context.meta, meta);
                }
                std::mem::swap(&mut context.extensions, &mut logging.extensions);

                if let Err(error) = service
                    .handle_notification(notification.notification, context)
                    .await
                {
                    tracing::warn!(?error, "Handle logging before handshake failed.");
                }
            }
            // Server could send pings before handshake
            ServerJsonRpcMessage::Request(ref request)
                if matches!(request.request, ServerRequest::PingRequest(_)) =>
            {
                tracing::trace!("Received ping request. Ignored.")
            }
            // Server SHOULD NOT send any other messages before handshake. We ignore them anyway
            _ => tracing::warn!(?message, "Received unexpected message"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[expect(clippy::exhaustive_structs, reason = "intentionally exhaustive")]
pub struct RoleClient;

/// Select the first client-preferred protocol version supported by the server.
///
/// Returns `None` when no version is shared.
pub fn select_protocol_version(
    client_preference: &[ProtocolVersion],
    server_supported: &[ProtocolVersion],
) -> Option<ProtocolVersion> {
    client_preference
        .iter()
        .find(|version| server_supported.contains(version))
        .cloned()
}

impl ServiceRole for RoleClient {
    type Req = ClientRequest;
    type Resp = ClientResult;
    type Not = ClientNotification;
    type PeerReq = ServerRequest;
    type PeerResp = ServerResult;
    type PeerNot = ServerNotification;
    type Info = ClientInfo;
    type PeerInfo = ServerInfo;
    type InitializeError = ClientInitializeError;
    const IS_CLIENT: bool = true;

    fn configure_direct_peer(peer: &Peer<Self>, info: &Self::Info) {
        let Some(server_info) = peer.peer_info() else {
            return;
        };
        if server_info.protocol_version.as_str() < ProtocolVersion::V_2026_07_28.as_str() {
            return;
        }
        peer.set_client_request_metadata(ClientRequestMetadata {
            protocol_version: server_info.protocol_version.clone(),
            client_info: info.client_info.clone(),
            client_capabilities: info.capabilities.clone(),
        });
    }

    fn peer_cancelled_params(notification: &Self::PeerNot) -> Option<&CancelledNotificationParam> {
        match notification {
            ServerNotification::CancelledNotification(notification) => Some(&notification.params),
            _ => None,
        }
    }

    // SEP-2260: with no outbound request in flight there is nothing the
    // server request could be associated with, so reject it. With one in
    // flight we cannot tell which request it belongs to (no wire field), so
    // we accept — an under-approximation of the spec's SHOULD.
    fn enforce_peer_request_association(
        peer_request: &Self::PeerReq,
        peer_info: Option<&Self::PeerInfo>,
        has_pending_outbound_request: bool,
    ) -> Result<(), ErrorData> {
        let restricted = matches!(
            peer_request,
            ServerRequest::CreateMessageRequest(_)
                | ServerRequest::ListRootsRequest(_)
                | ServerRequest::ElicitRequest(_)
        );
        if !restricted {
            return Ok(());
        }
        let strict =
            peer_info.is_some_and(|info| info.protocol_version >= ProtocolVersion::V_2026_07_28);
        if strict && !has_pending_outbound_request {
            return Err(ErrorData::invalid_params(
                "SEP-2260: server-to-client requests must be associated with an in-flight client request",
                None,
            ));
        }
        Ok(())
    }

    async fn invalidate_response_cache(peer: &Peer<Self>, notification: &Self::PeerNot) {
        match notification {
            ServerNotification::ResourceUpdatedNotification(notification) => {
                peer.invalidate_resource_read_cache(&notification.params.uri)
                    .await;
            }
            ServerNotification::ResourceListChangedNotification(_) => {
                peer.invalidate_resource_list_cache().await;
            }
            ServerNotification::ToolListChangedNotification(_) => {
                peer.invalidate_tool_cache().await;
            }
            ServerNotification::PromptListChangedNotification(_) => {
                peer.invalidate_prompt_cache().await;
            }
            _ => {}
        }
    }
}

pub type ServerSink = Peer<RoleClient>;

/// Default number of notifications buffered for one subscription.
pub const DEFAULT_SUBSCRIPTION_CHANNEL_CAPACITY: usize = 64;

/// How a client-side subscription stream ended.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum SubscriptionEnd {
    /// The server returned a final `SubscriptionsListenResult`.
    Graceful(SubscriptionsListenResult),
    /// The transport closed without a final result. Call `Peer::listen` again
    /// after reconnecting; subscription streams are not resumable.
    Abrupt,
    /// The subscription was explicitly cancelled by either peer.
    Cancelled,
    /// The consumer did not drain notifications before the channel filled.
    Lagged { capacity: usize },
}

/// Handle for one active `subscriptions/listen` request.
#[derive(Debug)]
pub struct Subscription {
    id: RequestId,
    acknowledged: SubscriptionFilter,
    notifications: tokio::sync::mpsc::Receiver<ServerNotification>,
    request: Option<RequestHandle<RoleClient>>,
    end: Option<SubscriptionEnd>,
}

type SubscriptionResponse =
    Result<Result<ServerResult, ServiceError>, tokio::sync::oneshot::error::RecvError>;

struct PendingSubscriptionRequest {
    handle: Option<RequestHandle<RoleClient>>,
}

impl PendingSubscriptionRequest {
    fn new(handle: RequestHandle<RoleClient>) -> Self {
        Self {
            handle: Some(handle),
        }
    }

    async fn recv(&mut self) -> Option<SubscriptionResponse> {
        let handle = self.handle.as_mut()?;
        Some((&mut handle.rx).await)
    }

    fn take(&mut self) -> Option<RequestHandle<RoleClient>> {
        self.handle.take()
    }

    fn unregister(&self, id: &RequestId) {
        if let Some(handle) = self.handle.as_ref() {
            handle.peer.unregister_subscription(id);
        }
    }

    fn disarm(&mut self) {
        self.handle.take();
    }

    async fn cancel(&mut self, reason: &'static str) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.cancel(Some(reason.to_owned())).await;
        }
    }
}

impl Drop for PendingSubscriptionRequest {
    fn drop(&mut self) {
        let Some(handle) = self.handle.take() else {
            return;
        };
        handle.peer.unregister_subscription(&handle.id);
        handle.peer.try_cancel_request(
            handle.id,
            Some("subscription establishment cancelled".to_owned()),
        );
    }
}

impl Subscription {
    /// Return the originating listen request ID.
    pub fn id(&self) -> &RequestId {
        &self.id
    }

    /// Return the notification filter accepted by the server.
    pub fn acknowledged(&self) -> &SubscriptionFilter {
        &self.acknowledged
    }

    /// Return the terminal state after this subscription has ended.
    pub fn end(&self) -> Option<&SubscriptionEnd> {
        self.end.as_ref()
    }

    /// Receive the next notification, or `None` after the subscription ends.
    ///
    /// A graceful final result and an abrupt transport close are distinguished
    /// through [`Self::end`].
    ///
    /// # Errors
    ///
    /// Returns a service or protocol error when the stream carries an invalid
    /// message, an unexpected final result, or another request failure.
    pub async fn next(&mut self) -> Result<Option<ServerNotification>, ServiceError> {
        if self.end.is_some() {
            return Ok(None);
        }
        let Some(request) = self.request.as_mut() else {
            self.end = Some(SubscriptionEnd::Abrupt);
            return Ok(None);
        };

        tokio::select! {
            biased;
            notification = self.notifications.recv() => {
                let Some(notification) = notification else {
                    let response = (&mut request.rx).await;
                    return self.handle_response(response);
                };
                if let ServerNotification::CancelledNotification(cancelled) = &notification {
                    if cancelled.params.request_id.as_ref() != Some(&self.id) {
                        self.cancel_as_abrupt("subscription cancellation ID mismatch")
                            .await;
                        return Err(ServiceError::UnexpectedResponse);
                    }
                    self.finish(SubscriptionEnd::Cancelled);
                    return Ok(None);
                }
                if notification.get_meta().subscription_id().as_ref() != Some(&self.id) {
                    self.cancel_as_abrupt("subscription notification ID mismatch")
                        .await;
                    return Err(ServiceError::UnexpectedResponse);
                }
                if !self.accepts(&notification) {
                    self.cancel_as_abrupt(
                        "subscription notification was outside the acknowledged filter",
                    )
                    .await;
                    return Err(ServiceError::UnexpectedResponse);
                }
                Ok(Some(notification))
            }
            response = &mut request.rx => {
                self.handle_response(response)
            }
        }
    }

    /// Cancel this subscription.
    ///
    /// # Errors
    ///
    /// Returns a transport error when the cancellation signal cannot be sent.
    pub async fn cancel(&mut self) -> Result<(), ServiceError> {
        self.cancel_with_reason(None).await
    }

    /// Cancel this subscription with a diagnostic reason.
    ///
    /// # Errors
    ///
    /// Returns a transport error when the cancellation signal cannot be sent.
    pub async fn cancel_with_reason(&mut self, reason: Option<String>) -> Result<(), ServiceError> {
        let Some(request) = self.request.take() else {
            return Ok(());
        };
        request.cancel(reason).await?;
        self.end = Some(SubscriptionEnd::Cancelled);
        Ok(())
    }

    fn finish(&mut self, end: SubscriptionEnd) {
        if let Some(request) = self.request.take() {
            request.peer.unregister_subscription(&self.id);
        }
        self.end = Some(end);
    }

    async fn cancel_as_abrupt(&mut self, reason: &'static str) {
        if let Some(request) = self.request.take() {
            let _ = request.cancel(Some(reason.to_owned())).await;
        }
        self.end = Some(SubscriptionEnd::Abrupt);
    }

    fn accepts(&self, notification: &ServerNotification) -> bool {
        match notification {
            ServerNotification::ToolListChangedNotification(_) => {
                self.acknowledged.tools_list_changed == Some(true)
            }
            ServerNotification::PromptListChangedNotification(_) => {
                self.acknowledged.prompts_list_changed == Some(true)
            }
            ServerNotification::ResourceListChangedNotification(_) => {
                self.acknowledged.resources_list_changed == Some(true)
            }
            ServerNotification::ResourceUpdatedNotification(update) => self
                .acknowledged
                .resource_subscriptions
                .as_ref()
                .is_some_and(|uris| uris.contains(&update.params.uri)),
            ServerNotification::SubscriptionsAcknowledgedNotification(_)
            | ServerNotification::CancelledNotification(_)
            | ServerNotification::ProgressNotification(_)
            | ServerNotification::LoggingMessageNotification(_)
            | ServerNotification::TaskStatusNotification(_)
            | ServerNotification::CustomNotification(_) => false,
        }
    }

    fn handle_response(
        &mut self,
        response: SubscriptionResponse,
    ) -> Result<Option<ServerNotification>, ServiceError> {
        let response = match response {
            Ok(response) => response,
            Err(_) => {
                self.finish(SubscriptionEnd::Abrupt);
                return Ok(None);
            }
        };
        let response = match response {
            Ok(response) => response,
            Err(ServiceError::TransportClosed) => {
                self.finish(SubscriptionEnd::Abrupt);
                return Ok(None);
            }
            Err(ServiceError::SubscriptionLagged { capacity }) => {
                self.finish(SubscriptionEnd::Lagged { capacity });
                return Ok(None);
            }
            Err(error) => {
                self.finish(SubscriptionEnd::Abrupt);
                return Err(error);
            }
        };
        let ServerResult::SubscriptionsListenResult(result) = response else {
            self.finish(SubscriptionEnd::Abrupt);
            return Err(ServiceError::UnexpectedResponse);
        };
        if !result.result_type.is_complete()
            || result.meta.subscription_id().as_ref() != Some(&self.id)
        {
            self.finish(SubscriptionEnd::Abrupt);
            return Err(ServiceError::UnexpectedResponse);
        }
        self.finish(SubscriptionEnd::Graceful(result));
        Ok(None)
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let Some(request) = self.request.take() else {
            return;
        };
        request.peer.unregister_subscription(&self.id);
        request.peer.try_cancel_request(
            self.id.clone(),
            Some("subscription handle dropped".to_owned()),
        );
    }
}

/// Selects how a client establishes its MCP lifecycle.
///
/// Existing [`ServiceExt::serve`] behavior remains legacy initialization.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClientLifecycleMode {
    /// Use the legacy `initialize` / `notifications/initialized` handshake.
    Initialize,
    /// Use `server/discover` and send self-contained per-request metadata.
    Discover {
        preferred_versions: Vec<ProtocolVersion>,
    },
    /// Probe with `server/discover`, falling back only when the peer proves it is legacy.
    Auto {
        preferred_versions: Vec<ProtocolVersion>,
        legacy_version: Option<ProtocolVersion>,
    },
}

/// Client-specific lifecycle entry points.
pub trait ClientServiceExt: Service<RoleClient> + Sized {
    fn serve_with_lifecycle<T, E, A>(
        self,
        transport: T,
        lifecycle: ClientLifecycleMode,
    ) -> impl Future<Output = Result<RunningService<RoleClient, Self>, ClientInitializeError>>
    + MaybeSendFuture
    where
        T: IntoTransport<RoleClient, E, A>,
        E: std::error::Error + Send + Sync + 'static,
    {
        serve_client_with_lifecycle(self, transport, lifecycle)
    }
}

impl<S: Service<RoleClient>> ClientServiceExt for S {}

impl<S: Service<RoleClient>> ServiceExt<RoleClient> for S {
    fn serve_with_ct<T, E, A>(
        self,
        transport: T,
        ct: CancellationToken,
    ) -> impl Future<Output = Result<RunningService<RoleClient, Self>, ClientInitializeError>>
    + MaybeSendFuture
    where
        T: IntoTransport<RoleClient, E, A>,
        E: std::error::Error + Send + Sync + 'static,
        Self: Sized,
    {
        serve_client_with_ct(self, transport, ct)
    }
}

pub async fn serve_client<S, T, E, A>(
    service: S,
    transport: T,
) -> Result<RunningService<RoleClient, S>, ClientInitializeError>
where
    S: Service<RoleClient>,
    T: IntoTransport<RoleClient, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    serve_client_with_lifecycle_and_ct(
        service,
        transport,
        ClientLifecycleMode::Initialize,
        Default::default(),
    )
    .await
}

pub async fn serve_client_with_ct<S, T, E, A>(
    service: S,
    transport: T,
    ct: CancellationToken,
) -> Result<RunningService<RoleClient, S>, ClientInitializeError>
where
    S: Service<RoleClient>,
    T: IntoTransport<RoleClient, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    serve_client_with_lifecycle_and_ct(service, transport, ClientLifecycleMode::Initialize, ct)
        .await
}

pub async fn serve_client_with_lifecycle<S, T, E, A>(
    service: S,
    transport: T,
    lifecycle: ClientLifecycleMode,
) -> Result<RunningService<RoleClient, S>, ClientInitializeError>
where
    S: Service<RoleClient>,
    T: IntoTransport<RoleClient, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    serve_client_with_lifecycle_and_ct(service, transport, lifecycle, Default::default()).await
}

pub async fn serve_client_with_lifecycle_and_ct<S, T, E, A>(
    service: S,
    transport: T,
    lifecycle: ClientLifecycleMode,
    ct: CancellationToken,
) -> Result<RunningService<RoleClient, S>, ClientInitializeError>
where
    S: Service<RoleClient>,
    T: IntoTransport<RoleClient, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    tokio::select! {
        result = serve_client_with_ct_inner(service, transport.into_transport(), lifecycle, ct.clone()) => { result }
        _ = ct.cancelled() => {
            Err(ClientInitializeError::Cancelled)
        }
    }
}

async fn serve_client_with_ct_inner<S, T>(
    service: S,
    transport: T,
    lifecycle: ClientLifecycleMode,
    ct: CancellationToken,
) -> Result<RunningService<RoleClient, S>, ClientInitializeError>
where
    S: Service<RoleClient>,
    T: Transport<RoleClient> + 'static,
{
    let mut transport = transport.into_transport();
    let id_provider = <Arc<AtomicU32RequestIdProvider>>::default();
    let (peer, peer_rx) = Peer::new(id_provider.clone(), None);
    let client_info = service.get_info();

    match lifecycle {
        ClientLifecycleMode::Initialize => {
            legacy_startup(&service, &mut transport, &id_provider, &peer, client_info).await?;
        }
        ClientLifecycleMode::Discover { preferred_versions } => {
            discover_startup(
                &service,
                &mut transport,
                &id_provider,
                &peer,
                &client_info,
                preferred_versions,
            )
            .await?;
        }
        ClientLifecycleMode::Auto {
            preferred_versions,
            legacy_version,
        } => {
            let discover_result = discover_startup(
                &service,
                &mut transport,
                &id_provider,
                &peer,
                &client_info,
                preferred_versions,
            )
            .await;
            match discover_result {
                Ok(()) => {}
                Err(ClientInitializeError::JsonRpcError(error))
                    if error.code == crate::model::ErrorCode::METHOD_NOT_FOUND =>
                {
                    let mut legacy_info = client_info;
                    if let Some(version) = legacy_version {
                        legacy_info.protocol_version = version;
                    }
                    legacy_startup(&service, &mut transport, &id_provider, &peer, legacy_info)
                        .await?;
                }
                Err(error) => return Err(error),
            }
        }
    }
    Ok(serve_inner(service, transport, peer, peer_rx, ct))
}

async fn legacy_startup<S, T>(
    service: &S,
    transport: &mut T,
    id_provider: &Arc<AtomicU32RequestIdProvider>,
    peer: &Peer<RoleClient>,
    client_info: ClientInfo,
) -> Result<(), ClientInitializeError>
where
    S: Service<RoleClient>,
    T: Transport<RoleClient> + 'static,
{
    let id = id_provider.next_request_id();
    let init_request = InitializeRequest {
        method: Default::default(),
        params: client_info,
        extensions: Default::default(),
    };
    transport
        .send(ClientJsonRpcMessage::request(
            ClientRequest::InitializeRequest(init_request),
            id.clone(),
        ))
        .await
        .map_err(|error| ClientInitializeError::TransportError {
            error: DynamicTransportError::new::<T, _>(error),
            context: "send initialize request".into(),
        })?;

    let (response, response_id) =
        expect_response(transport, "initialize response", service, peer.clone()).await?;

    if !id.matches_response_id(&response_id) {
        return Err(ClientInitializeError::ConflictInitResponseId(
            id,
            response_id,
        ));
    }

    let ServerResult::InitializeResult(initialize_result) = response else {
        return Err(ClientInitializeError::ExpectedInitResult(Some(response)));
    };
    peer.set_peer_info(initialize_result);

    // send notification
    let notification = ClientJsonRpcMessage::notification(
        ClientNotification::InitializedNotification(InitializedNotification {
            method: Default::default(),
            extensions: Default::default(),
        }),
    );
    transport.send(notification).await.map_err(|error| {
        ClientInitializeError::transport::<T>(error, "send initialized notification")
    })?;
    Ok(())
}

async fn discover_startup<S, T>(
    service: &S,
    transport: &mut T,
    id_provider: &Arc<AtomicU32RequestIdProvider>,
    peer: &Peer<RoleClient>,
    client_info: &ClientInfo,
    preferred_versions: Vec<ProtocolVersion>,
) -> Result<(), ClientInitializeError>
where
    S: Service<RoleClient>,
    T: Transport<RoleClient> + 'static,
{
    if preferred_versions.is_empty() {
        return Err(ClientInitializeError::NoPreferredProtocolVersion);
    }

    let mut attempted = Vec::new();
    let mut candidate = preferred_versions[0].clone();
    loop {
        attempted.push(candidate.clone());

        let meta = RequestMetaObject::with_client_context(
            candidate.clone(),
            client_info.client_info.clone(),
            client_info.capabilities.clone(),
        );
        let mut discover = DiscoverRequest::new(DiscoverRequestParams {});
        discover.extensions.insert(meta);
        let id = id_provider.next_request_id();
        transport
            .send(ClientJsonRpcMessage::request(
                ClientRequest::DiscoverRequest(discover),
                id.clone(),
            ))
            .await
            .map_err(|error| {
                ClientInitializeError::transport::<T>(error, "send discover request")
            })?;

        match expect_response(transport, "discover response", service, peer.clone()).await {
            Ok((ServerResult::DiscoverResult(result), response_id)) => {
                if !id.matches_response_id(&response_id) {
                    return Err(ClientInitializeError::ConflictInitResponseId(
                        id,
                        response_id,
                    ));
                }
                let Some(selected) =
                    select_protocol_version(&preferred_versions, &result.supported_versions)
                else {
                    return Err(ClientInitializeError::NoCompatibleProtocolVersion {
                        client_supported: preferred_versions,
                        server_supported: result.supported_versions,
                    });
                };
                peer.set_peer_info(ServerInfo {
                    protocol_version: selected.clone(),
                    capabilities: result.capabilities,
                    server_info: result.server_info,
                    instructions: result.instructions,
                    meta: result.meta,
                });
                peer.set_client_request_metadata(ClientRequestMetadata {
                    protocol_version: selected,
                    client_info: client_info.client_info.clone(),
                    client_capabilities: client_info.capabilities.clone(),
                });
                return Ok(());
            }
            Ok((response, _)) => {
                return Err(ClientInitializeError::ExpectedInitResult(Some(response)));
            }
            Err(ClientInitializeError::JsonRpcError(error))
                if error.code == crate::model::ErrorCode::UNSUPPORTED_PROTOCOL_VERSION =>
            {
                let supported = error
                    .data
                    .as_ref()
                    .and_then(|data| data.get("supported"))
                    .cloned()
                    .and_then(|value| serde_json::from_value::<Vec<ProtocolVersion>>(value).ok())
                    .unwrap_or_default();
                let may_retry_current = attempted
                    .iter()
                    .filter(|version| *version == &candidate)
                    .count()
                    == 1;
                let next = preferred_versions
                    .iter()
                    .find(|version| {
                        supported.contains(version)
                            && (!attempted.contains(version)
                                || (may_retry_current && *version == &candidate))
                    })
                    .cloned();
                let Some(next) = next else {
                    return Err(ClientInitializeError::NoCompatibleProtocolVersion {
                        client_supported: preferred_versions,
                        server_supported: supported,
                    });
                };
                candidate = next;
            }
            Err(error) => return Err(error),
        }
    }
}

const DISCOVER_CACHE_PREFIX: &str = "server/discover:";
const TOOL_LIST_CACHE_PREFIX: &str = "tools/list:";
const PROMPT_LIST_CACHE_PREFIX: &str = "prompts/list:";
const RESOURCE_LIST_CACHE_PREFIX: &str = "resources/list:";
const RESOURCE_TEMPLATE_LIST_CACHE_PREFIX: &str = "resources/templates/list:";
const RESOURCE_READ_CACHE_PREFIX: &str = "resources/read:";

// Cache keys are built only from the request method plus the parameters that
// affect the result (SEP-2549). Request `_meta` (progress tokens, trace
// context, etc.) does not affect the result, so it is deliberately excluded to
// avoid fragmenting the cache across otherwise-identical requests.
fn discover_cache_key() -> String {
    // `server/discover` carries no result-affecting parameters.
    DISCOVER_CACHE_PREFIX.to_string()
}

fn list_response_cache_key(prefix: &str, params: &Option<PaginatedRequestParams>) -> String {
    // Only the pagination cursor affects which page is returned.
    let cursor = params.as_ref().and_then(|params| params.cursor.as_deref());
    let cursor =
        serde_json::to_string(&cursor).expect("serializing a pagination cursor cannot fail");
    format!("{prefix}{cursor}")
}

fn resource_read_cache_key(params: &ReadResourceRequestParams) -> Option<String> {
    // MRTR retries depend on inputs that are not part of the cache key and MUST
    // NOT be cached.
    if params.input_responses.is_some() || params.request_state.is_some() {
        return None;
    }
    // Only the URI affects the result.
    Some(resource_read_cache_prefix_for_uri(&params.uri))
}

fn resource_read_cache_prefix_for_uri(uri: &str) -> String {
    let uri = serde_json::to_string(uri).expect("serializing a resource URI cannot fail");
    format!("{RESOURCE_READ_CACHE_PREFIX}{uri}:")
}

fn request_uses_cursor(params: &Option<PaginatedRequestParams>) -> bool {
    params
        .as_ref()
        .and_then(|params| params.cursor.as_ref())
        .is_some()
}

macro_rules! method {
    ($(#[$meta:meta])* peer_req $method:ident $Req:ident() => $Resp: ident ) => {
        $(#[$meta])*
        pub async fn $method(&self) -> Result<$Resp, ServiceError> {
            let result = self
                .send_request(ClientRequest::$Req($Req {
                    method: Default::default(),
                }))
                .await?;
            match result {
                ServerResult::$Resp(result) => Ok(result),
                _ => Err(ServiceError::UnexpectedResponse),
            }
        }
    };
    ($(#[$meta:meta])* peer_req $method:ident $Req:ident($Param: ident) => $Resp: ident ) => {
        $(#[$meta])*
        pub async fn $method(&self, params: $Param) -> Result<$Resp, ServiceError> {
            let result = self
                .send_request(ClientRequest::$Req($Req {
                    method: Default::default(),
                    params,
                    extensions: Default::default(),
                }))
                .await?;
            match result {
                ServerResult::$Resp(result) => Ok(result),
                _ => Err(ServiceError::UnexpectedResponse),
            }
        }
    };
    ($(#[$meta:meta])* peer_req $method:ident $Req:ident($Param: ident)? => $Resp: ident ) => {
        $(#[$meta])*
        pub async fn $method(&self, params: Option<$Param>) -> Result<$Resp, ServiceError> {
            let result = self
                .send_request(ClientRequest::$Req($Req {
                    method: Default::default(),
                    params,
                    extensions: Default::default(),
                }))
                .await?;
            match result {
                ServerResult::$Resp(result) => Ok(result),
                _ => Err(ServiceError::UnexpectedResponse),
            }
        }
    };
    ($(#[$meta:meta])* peer_req $method:ident $Req:ident($Param: ident)) => {
        $(#[$meta])*
        pub async fn $method(&self, params: $Param) -> Result<(), ServiceError> {
            let result = self
                .send_request(ClientRequest::$Req($Req {
                    method: Default::default(),
                    params,
                    extensions: Default::default(),
                }))
                .await?;
            match result {
                ServerResult::EmptyResult(_) => Ok(()),
                _ => Err(ServiceError::UnexpectedResponse),
            }
        }
    };

    ($(#[$meta:meta])* peer_not $method:ident $Not:ident($Param: ident)) => {
        $(#[$meta])*
        pub async fn $method(&self, params: $Param) -> Result<(), ServiceError> {
            self.send_notification(ClientNotification::$Not($Not {
                method: Default::default(),
                params,
                extensions: Default::default(),
            }))
            .await?;
            Ok(())
        }
    };
    ($(#[$meta:meta])* peer_not $method:ident $Not:ident) => {
        $(#[$meta])*
        pub async fn $method(&self) -> Result<(), ServiceError> {
            self.send_notification(ClientNotification::$Not($Not {
                method: Default::default(),
                extensions: Default::default(),
            }))
            .await?;
            Ok(())
        }
    };
}

impl Peer<RoleClient> {
    /// Open a long-lived notification subscription and wait for its acknowledgment.
    ///
    /// Notifications routed to the returned [`Subscription`] are not also
    /// delivered through [`ClientHandler`](crate::ClientHandler) callbacks.
    ///
    /// # Errors
    ///
    /// Returns a service, transport, or protocol error when the request cannot
    /// be established or the acknowledgment is invalid.
    pub async fn listen(
        &self,
        notifications: SubscriptionFilter,
    ) -> Result<Subscription, ServiceError> {
        self.listen_with_channel_capacity_inner(
            notifications,
            DEFAULT_SUBSCRIPTION_CHANNEL_CAPACITY,
        )
        .await
    }

    /// Open a subscription with an explicit notification buffer capacity.
    ///
    /// Notifications routed to the returned [`Subscription`] are not also
    /// delivered through [`ClientHandler`](crate::ClientHandler) callbacks.
    ///
    /// # Errors
    ///
    /// Returns a service, transport, or protocol error when the request cannot
    /// be established or the acknowledgment is invalid.
    pub async fn listen_with_capacity(
        &self,
        notifications: SubscriptionFilter,
        channel_capacity: NonZeroUsize,
    ) -> Result<Subscription, ServiceError> {
        self.listen_with_channel_capacity_inner(notifications, channel_capacity.get())
            .await
    }

    async fn listen_with_channel_capacity_inner(
        &self,
        notifications: SubscriptionFilter,
        channel_capacity: usize,
    ) -> Result<Subscription, ServiceError> {
        let request = ClientRequest::SubscriptionsListenRequest(SubscriptionsListenRequest::new(
            SubscriptionsListenRequestParams::new(notifications.clone()),
        ));
        let (handle, mut subscription_notifications) = self
            .send_subscription_request(request, PeerRequestOptions::no_options(), channel_capacity)
            .await?;
        let id = handle.id.clone();
        let mut pending = PendingSubscriptionRequest::new(handle);

        tokio::select! {
            biased;
            notification = subscription_notifications.recv() => {
                let Some(notification) = notification else {
                    pending.cancel("subscription stream closed before acknowledgment").await;
                    return Err(ServiceError::TransportClosed);
                };
                if notification.get_meta().subscription_id().as_ref() != Some(&id) {
                    pending.cancel("subscription acknowledgment ID mismatch").await;
                    return Err(ServiceError::UnexpectedResponse);
                }
                let ServerNotification::SubscriptionsAcknowledgedNotification(
                    acknowledgment,
                ) = notification else {
                    pending.cancel("notification received before subscription acknowledgment").await;
                    return Err(ServiceError::UnexpectedResponse);
                };
                let accepted = acknowledgment.params.notifications;
                if !accepted.is_subset_of(&notifications) {
                    pending.cancel("subscription acknowledged an unrequested filter").await;
                    return Err(ServiceError::UnexpectedResponse);
                }
                let Some(handle) = pending.take() else {
                    return Err(ServiceError::TransportClosed);
                };
                Ok(Subscription {
                    id,
                    acknowledged: accepted,
                    notifications: subscription_notifications,
                    request: Some(handle),
                    end: None,
                })
            }
            response = pending.recv() => {
                pending.unregister(&id);
                pending.disarm();
                let Some(response) = response else {
                    return Err(ServiceError::TransportClosed);
                };
                match response {
                    Ok(Err(error)) => Err(error),
                    Ok(Ok(_)) => Err(ServiceError::UnexpectedResponse),
                    Err(_) => Err(ServiceError::TransportClosed),
                }
            }
        }
    }

    /// Discover the server's supported protocol versions and capabilities.
    ///
    /// The high-level client currently exposes this peer only after initialization;
    /// pre-initialization probing is planned as follow-up work.
    pub async fn discover(&self, meta: RequestMetaObject) -> Result<DiscoverResult, ServiceError> {
        let cache_key = discover_cache_key();
        if let Some(ServerResult::DiscoverResult(result)) = self.cached_response(&cache_key).await {
            return Ok(result);
        }
        let generation = self.capture_response_cache_generation().await;
        let mut request = DiscoverRequest::new(DiscoverRequestParams {});
        request.extensions.insert(meta);
        let result = self
            .send_request(ClientRequest::DiscoverRequest(request))
            .await;
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                if let Some(ServerResult::DiscoverResult(result)) =
                    self.stale_cached_response(&cache_key).await
                {
                    return Ok(result);
                }
                return Err(error);
            }
        };
        match result {
            ServerResult::DiscoverResult(result) => {
                self.cache_result(
                    Some(cache_key),
                    Some(result.ttl_ms),
                    Some(result.cache_scope),
                    generation,
                    ServerResult::DiscoverResult(result.clone()),
                )
                .await;
                Ok(result)
            }
            _ => Err(ServiceError::UnexpectedResponse),
        }
    }

    async fn cache_result(
        &self,
        cache_key: Option<String>,
        ttl_ms: Option<u64>,
        cache_scope: Option<CacheScope>,
        generation: CacheGeneration,
        result: ServerResult,
    ) {
        let Some(cache_key) = cache_key else {
            return;
        };
        self.cache_response_with_generation(cache_key, result, ttl_ms, cache_scope, generation)
            .await;
    }

    pub(crate) async fn invalidate_tool_cache(&self) {
        self.invalidate_cached_responses(TOOL_LIST_CACHE_PREFIX)
            .await;
    }

    pub(crate) async fn invalidate_prompt_cache(&self) {
        self.invalidate_cached_responses(PROMPT_LIST_CACHE_PREFIX)
            .await;
    }

    pub(crate) async fn invalidate_resource_list_cache(&self) {
        self.invalidate_cached_responses(RESOURCE_LIST_CACHE_PREFIX)
            .await;
        self.invalidate_cached_responses(RESOURCE_TEMPLATE_LIST_CACHE_PREFIX)
            .await;
    }

    pub(crate) async fn invalidate_resource_read_cache(&self, uri: &str) {
        self.invalidate_cached_responses(&resource_read_cache_prefix_for_uri(uri))
            .await;
    }

    /// Send one `tools/call` request and return either a final result or an MRTR
    /// `InputRequiredResult` without driving any follow-up rounds.
    pub async fn call_tool_once(
        &self,
        params: CallToolRequestParams,
    ) -> Result<CallToolResponse, ServiceError> {
        let result = self
            .send_request(ClientRequest::CallToolRequest(CallToolRequest {
                method: Default::default(),
                params,
                extensions: Default::default(),
            }))
            .await?;
        match result {
            ServerResult::CallToolResult(result) => Ok(CallToolResponse::Complete(result)),
            ServerResult::InputRequiredResult(result) => {
                Ok(CallToolResponse::InputRequired(result))
            }
            // SEP-2663 Tasks extension: the server materialized a task.
            ServerResult::CreateTaskResult(result) => Ok(CallToolResponse::Task(result)),
            _ => Err(ServiceError::UnexpectedResponse),
        }
    }

    /// SEP-2663 `tasks/get`: poll the current state of a task.
    pub async fn get_task(&self, params: GetTaskParams) -> Result<GetTaskResult, ServiceError> {
        let result = self
            .send_request(ClientRequest::GetTaskRequest(GetTaskRequest::new(params)))
            .await?;
        match result {
            ServerResult::GetTaskResult(result) => Ok(result),
            _ => Err(ServiceError::UnexpectedResponse),
        }
    }

    /// SEP-2663 `tasks/update`: deliver responses to outstanding in-task
    /// input requests. The acknowledgement is eventually consistent.
    pub async fn update_task(&self, params: UpdateTaskParams) -> Result<(), ServiceError> {
        let result = self
            .send_request(ClientRequest::UpdateTaskRequest(UpdateTaskRequest::new(
                params,
            )))
            .await?;
        match result {
            ServerResult::TaskAckResult(_) | ServerResult::EmptyResult(_) => Ok(()),
            _ => Err(ServiceError::UnexpectedResponse),
        }
    }

    /// SEP-2663 `tasks/cancel`: signal intent to cancel a task. Cancellation
    /// is cooperative; the ack does not guarantee the task stops.
    pub async fn cancel_task(&self, params: CancelTaskParams) -> Result<(), ServiceError> {
        let result = self
            .send_request(ClientRequest::CancelTaskRequest(CancelTaskRequest::new(
                params,
            )))
            .await?;
        match result {
            ServerResult::TaskAckResult(_) | ServerResult::EmptyResult(_) => Ok(()),
            _ => Err(ServiceError::UnexpectedResponse),
        }
    }

    /// Send one `prompts/get` request and return either a final result or an MRTR
    /// `InputRequiredResult` without driving any follow-up rounds.
    pub async fn get_prompt_once(
        &self,
        params: GetPromptRequestParams,
    ) -> Result<GetPromptResponse, ServiceError> {
        let result = self
            .send_request(ClientRequest::GetPromptRequest(GetPromptRequest {
                method: Default::default(),
                params,
                extensions: Default::default(),
            }))
            .await?;
        match result {
            ServerResult::GetPromptResult(result) => Ok(GetPromptResponse::Complete(result)),
            ServerResult::InputRequiredResult(result) => {
                Ok(GetPromptResponse::InputRequired(result))
            }
            _ => Err(ServiceError::UnexpectedResponse),
        }
    }

    /// Send one `resources/read` request and return either a final result or an
    /// MRTR `InputRequiredResult` without driving any follow-up rounds.
    pub async fn read_resource_once(
        &self,
        params: ReadResourceRequestParams,
    ) -> Result<ReadResourceResponse, ServiceError> {
        let cache_key = resource_read_cache_key(&params);
        if let Some(key) = cache_key.as_deref()
            && let Some(ServerResult::ReadResourceResult(result)) = self.cached_response(key).await
        {
            return Ok(ReadResourceResponse::Complete(result));
        }

        let generation = self.capture_response_cache_generation().await;
        let result = self
            .send_request(ClientRequest::ReadResourceRequest(ReadResourceRequest {
                method: Default::default(),
                params,
                extensions: Default::default(),
            }))
            .await;
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                if let Some(key) = cache_key.as_deref()
                    && let Some(ServerResult::ReadResourceResult(result)) =
                        self.stale_cached_response(key).await
                {
                    return Ok(ReadResourceResponse::Complete(result));
                }
                return Err(error);
            }
        };
        match result {
            ServerResult::ReadResourceResult(result) => {
                self.cache_result(
                    cache_key,
                    result.ttl_ms,
                    result.cache_scope,
                    generation,
                    ServerResult::ReadResourceResult(result.clone()),
                )
                .await;
                Ok(ReadResourceResponse::Complete(result))
            }
            ServerResult::InputRequiredResult(result) => {
                Ok(ReadResourceResponse::InputRequired(result))
            }
            _ => Err(ServiceError::UnexpectedResponse),
        }
    }

    method!(peer_req complete CompleteRequest(CompleteRequestParams) => CompleteResult);
    method!(
        #[deprecated(
            since = "1.8.0",
            note = "Logging is deprecated by SEP-2577 and will be removed in a future release. See https://github.com/modelcontextprotocol/modelcontextprotocol/pull/2577"
        )]
        peer_req set_level SetLevelRequest(SetLevelRequestParams)
    );
    method!(peer_req get_prompt GetPromptRequest(GetPromptRequestParams) => GetPromptResult);
    method!(
        #[deprecated(
            note = "resources/subscribe is legacy-only; use Peer::listen for protocol version 2026-07-28"
        )]
        peer_req subscribe SubscribeRequest(SubscribeRequestParams)
    );
    method!(
        #[deprecated(
            note = "resources/unsubscribe is legacy-only; cancel the Subscription handle instead"
        )]
        peer_req unsubscribe UnsubscribeRequest(UnsubscribeRequestParams)
    );
    method!(peer_req call_tool CallToolRequest(CallToolRequestParams) => CallToolResult);

    pub async fn list_prompts(
        &self,
        params: Option<PaginatedRequestParams>,
    ) -> Result<ListPromptsResult, ServiceError> {
        let cache_key = list_response_cache_key(PROMPT_LIST_CACHE_PREFIX, &params);
        if let Some(ServerResult::ListPromptsResult(result)) =
            self.cached_response(&cache_key).await
        {
            return Ok(result);
        }
        let generation = self.capture_response_cache_generation().await;
        let uses_cursor = request_uses_cursor(&params);
        let result = self
            .send_request(ClientRequest::ListPromptsRequest(ListPromptsRequest {
                method: Default::default(),
                params,
                extensions: Default::default(),
            }))
            .await;
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                if uses_cursor {
                    self.invalidate_prompt_cache().await;
                    return Err(error);
                }
                if let Some(ServerResult::ListPromptsResult(result)) =
                    self.stale_cached_response(&cache_key).await
                {
                    return Ok(result);
                }
                return Err(error);
            }
        };
        match result {
            ServerResult::ListPromptsResult(result) => {
                self.cache_result(
                    Some(cache_key),
                    result.ttl_ms,
                    result.cache_scope,
                    generation,
                    ServerResult::ListPromptsResult(result.clone()),
                )
                .await;
                Ok(result)
            }
            _ => Err(ServiceError::UnexpectedResponse),
        }
    }

    pub async fn list_resources(
        &self,
        params: Option<PaginatedRequestParams>,
    ) -> Result<ListResourcesResult, ServiceError> {
        let cache_key = list_response_cache_key(RESOURCE_LIST_CACHE_PREFIX, &params);
        if let Some(ServerResult::ListResourcesResult(result)) =
            self.cached_response(&cache_key).await
        {
            return Ok(result);
        }
        let generation = self.capture_response_cache_generation().await;
        let uses_cursor = request_uses_cursor(&params);
        let result = self
            .send_request(ClientRequest::ListResourcesRequest(ListResourcesRequest {
                method: Default::default(),
                params,
                extensions: Default::default(),
            }))
            .await;
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                if uses_cursor {
                    self.invalidate_cached_responses(RESOURCE_LIST_CACHE_PREFIX)
                        .await;
                    return Err(error);
                }
                if let Some(ServerResult::ListResourcesResult(result)) =
                    self.stale_cached_response(&cache_key).await
                {
                    return Ok(result);
                }
                return Err(error);
            }
        };
        match result {
            ServerResult::ListResourcesResult(result) => {
                self.cache_result(
                    Some(cache_key),
                    result.ttl_ms,
                    result.cache_scope,
                    generation,
                    ServerResult::ListResourcesResult(result.clone()),
                )
                .await;
                Ok(result)
            }
            _ => Err(ServiceError::UnexpectedResponse),
        }
    }

    pub async fn list_resource_templates(
        &self,
        params: Option<PaginatedRequestParams>,
    ) -> Result<ListResourceTemplatesResult, ServiceError> {
        let cache_key = list_response_cache_key(RESOURCE_TEMPLATE_LIST_CACHE_PREFIX, &params);
        if let Some(ServerResult::ListResourceTemplatesResult(result)) =
            self.cached_response(&cache_key).await
        {
            return Ok(result);
        }
        let generation = self.capture_response_cache_generation().await;
        let uses_cursor = request_uses_cursor(&params);
        let result = self
            .send_request(ClientRequest::ListResourceTemplatesRequest(
                ListResourceTemplatesRequest {
                    method: Default::default(),
                    params,
                    extensions: Default::default(),
                },
            ))
            .await;
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                if uses_cursor {
                    self.invalidate_cached_responses(RESOURCE_TEMPLATE_LIST_CACHE_PREFIX)
                        .await;
                    return Err(error);
                }
                if let Some(ServerResult::ListResourceTemplatesResult(result)) =
                    self.stale_cached_response(&cache_key).await
                {
                    return Ok(result);
                }
                return Err(error);
            }
        };
        match result {
            ServerResult::ListResourceTemplatesResult(result) => {
                self.cache_result(
                    Some(cache_key),
                    result.ttl_ms,
                    result.cache_scope,
                    generation,
                    ServerResult::ListResourceTemplatesResult(result.clone()),
                )
                .await;
                Ok(result)
            }
            _ => Err(ServiceError::UnexpectedResponse),
        }
    }

    pub async fn read_resource(
        &self,
        params: ReadResourceRequestParams,
    ) -> Result<ReadResourceResult, ServiceError> {
        match self.read_resource_once(params).await? {
            ReadResourceResponse::Complete(result) => Ok(result),
            ReadResourceResponse::InputRequired(_) => Err(ServiceError::UnexpectedResponse),
        }
    }

    pub async fn list_tools(
        &self,
        params: Option<PaginatedRequestParams>,
    ) -> Result<ListToolsResult, ServiceError> {
        let cache_key = list_response_cache_key(TOOL_LIST_CACHE_PREFIX, &params);
        if let Some(ServerResult::ListToolsResult(result)) = self.cached_response(&cache_key).await
        {
            return Ok(result);
        }
        let generation = self.capture_response_cache_generation().await;
        let uses_cursor = request_uses_cursor(&params);
        let result = self
            .send_request(ClientRequest::ListToolsRequest(ListToolsRequest {
                method: Default::default(),
                params,
                extensions: Default::default(),
            }))
            .await;
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                if uses_cursor {
                    self.invalidate_tool_cache().await;
                    return Err(error);
                }
                if let Some(ServerResult::ListToolsResult(result)) =
                    self.stale_cached_response(&cache_key).await
                {
                    return Ok(result);
                }
                return Err(error);
            }
        };
        match result {
            ServerResult::ListToolsResult(result) => {
                self.cache_result(
                    Some(cache_key),
                    result.ttl_ms,
                    result.cache_scope,
                    generation,
                    ServerResult::ListToolsResult(result.clone()),
                )
                .await;
                Ok(result)
            }
            _ => Err(ServiceError::UnexpectedResponse),
        }
    }

    method!(peer_not notify_cancelled CancelledNotification(CancelledNotificationParam));
    method!(peer_not notify_progress ProgressNotification(ProgressNotificationParam));
    method!(peer_not notify_initialized InitializedNotification);
    method!(peer_not notify_roots_list_changed RootsListChangedNotification);
}

impl Peer<RoleClient> {
    /// A wrapper method for [`Peer<RoleClient>::list_tools`].
    ///
    /// This function will call [`Peer<RoleClient>::list_tools`] multiple times until all tools are listed.
    pub async fn list_all_tools(&self) -> Result<Vec<crate::model::Tool>, ServiceError> {
        let mut tools = Vec::new();
        let mut cursor = None;
        loop {
            let result = self
                .list_tools(Some(PaginatedRequestParams { meta: None, cursor }))
                .await?;
            tools.extend(result.tools);
            cursor = result.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        Ok(tools)
    }

    /// A wrapper method for [`Peer<RoleClient>::list_prompts`].
    ///
    /// This function will call [`Peer<RoleClient>::list_prompts`] multiple times until all prompts are listed.
    pub async fn list_all_prompts(&self) -> Result<Vec<crate::model::Prompt>, ServiceError> {
        let mut prompts = Vec::new();
        let mut cursor = None;
        loop {
            let result = self
                .list_prompts(Some(PaginatedRequestParams { meta: None, cursor }))
                .await?;
            prompts.extend(result.prompts);
            cursor = result.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        Ok(prompts)
    }

    /// A wrapper method for [`Peer<RoleClient>::list_resources`].
    ///
    /// This function will call [`Peer<RoleClient>::list_resources`] multiple times until all resources are listed.
    pub async fn list_all_resources(&self) -> Result<Vec<crate::model::Resource>, ServiceError> {
        let mut resources = Vec::new();
        let mut cursor = None;
        loop {
            let result = self
                .list_resources(Some(PaginatedRequestParams { meta: None, cursor }))
                .await?;
            resources.extend(result.resources);
            cursor = result.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        Ok(resources)
    }

    /// A wrapper method for [`Peer<RoleClient>::list_resource_templates`].
    ///
    /// This function will call [`Peer<RoleClient>::list_resource_templates`] multiple times until all resource templates are listed.
    pub async fn list_all_resource_templates(
        &self,
    ) -> Result<Vec<crate::model::ResourceTemplate>, ServiceError> {
        let mut resource_templates = Vec::new();
        let mut cursor = None;
        loop {
            let result = self
                .list_resource_templates(Some(PaginatedRequestParams { meta: None, cursor }))
                .await?;
            resource_templates.extend(result.resource_templates);
            cursor = result.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        Ok(resource_templates)
    }

    /// Convenient method to get completion suggestions for a prompt argument
    ///
    /// # Arguments
    /// * `prompt_name` - Name of the prompt being completed
    /// * `argument_name` - Name of the argument being completed
    /// * `current_value` - Current partial value of the argument
    /// * `context` - Optional context with previously resolved arguments
    ///
    /// # Returns
    /// CompletionInfo with suggestions for the specified prompt argument
    pub async fn complete_prompt_argument(
        &self,
        prompt_name: impl Into<String>,
        argument_name: impl Into<String>,
        current_value: impl Into<String>,
        context: Option<CompletionContext>,
    ) -> Result<CompletionInfo, ServiceError> {
        let request = CompleteRequestParams {
            meta: None,
            r#ref: Reference::for_prompt(prompt_name),
            argument: ArgumentInfo {
                name: argument_name.into(),
                value: current_value.into(),
            },
            context,
        };

        let result = self.complete(request).await?;
        Ok(result.completion)
    }

    /// Convenient method to get completion suggestions for a resource URI argument
    ///
    /// # Arguments
    /// * `uri_template` - URI template pattern being completed
    /// * `argument_name` - Name of the URI parameter being completed
    /// * `current_value` - Current partial value of the parameter
    /// * `context` - Optional context with previously resolved arguments
    ///
    /// # Returns
    /// CompletionInfo with suggestions for the specified resource URI argument
    pub async fn complete_resource_argument(
        &self,
        uri_template: impl Into<String>,
        argument_name: impl Into<String>,
        current_value: impl Into<String>,
        context: Option<CompletionContext>,
    ) -> Result<CompletionInfo, ServiceError> {
        let request = CompleteRequestParams {
            meta: None,
            r#ref: Reference::for_resource(uri_template),
            argument: ArgumentInfo {
                name: argument_name.into(),
                value: current_value.into(),
            },
            context,
        };

        let result = self.complete(request).await?;
        Ok(result.completion)
    }

    /// Simple completion for a prompt argument without context
    ///
    /// This is a convenience wrapper around `complete_prompt_argument` for
    /// simple completion scenarios that don't require context awareness.
    pub async fn complete_prompt_simple(
        &self,
        prompt_name: impl Into<String>,
        argument_name: impl Into<String>,
        current_value: impl Into<String>,
    ) -> Result<Vec<String>, ServiceError> {
        let completion = self
            .complete_prompt_argument(prompt_name, argument_name, current_value, None)
            .await?;
        Ok(completion.values)
    }

    /// Simple completion for a resource URI argument without context
    ///
    /// This is a convenience wrapper around `complete_resource_argument` for
    /// simple completion scenarios that don't require context awareness.
    pub async fn complete_resource_simple(
        &self,
        uri_template: impl Into<String>,
        argument_name: impl Into<String>,
        current_value: impl Into<String>,
    ) -> Result<Vec<String>, ServiceError> {
        let completion = self
            .complete_resource_argument(uri_template, argument_name, current_value, None)
            .await?;
        Ok(completion.values)
    }
}

impl<S> RunningService<RoleClient, S>
where
    S: Service<RoleClient>,
{
    /// Send one `tools/call` request without driving MRTR follow-up rounds.
    pub async fn call_tool_once(
        &self,
        params: CallToolRequestParams,
    ) -> Result<CallToolResponse, ServiceError> {
        self.peer.call_tool_once(params).await
    }

    /// Send one `prompts/get` request without driving MRTR follow-up rounds.
    pub async fn get_prompt_once(
        &self,
        params: GetPromptRequestParams,
    ) -> Result<GetPromptResponse, ServiceError> {
        self.peer.get_prompt_once(params).await
    }

    /// Send one `resources/read` request without driving MRTR follow-up rounds.
    pub async fn read_resource_once(
        &self,
        params: ReadResourceRequestParams,
    ) -> Result<ReadResourceResponse, ServiceError> {
        self.peer.read_resource_once(params).await
    }

    /// High-level `tools/call` helper that automatically fulfils SEP-2322
    /// `input_required` rounds through the local [`ClientHandler`](crate::ClientHandler) service.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::InputRequiredRoundsExceeded`] if the peer does
    /// not produce a final [`CallToolResult`] within the default MRTR round cap.
    /// Other transport, protocol, and local input-handler errors are propagated.
    pub async fn call_tool(
        &self,
        params: CallToolRequestParams,
    ) -> Result<CallToolResult, ServiceError> {
        self.call_tool_with_mrtr_max_rounds(params, DEFAULT_MRTR_MAX_ROUNDS)
            .await
    }

    /// Same as [`Self::call_tool`], with an explicit MRTR round cap.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::InputRequiredRoundsExceeded`] once `max_rounds`
    /// `input_required` responses have been driven without receiving a final
    /// [`CallToolResult`]. Other transport, protocol, and local input-handler
    /// errors are propagated.
    pub async fn call_tool_with_mrtr_max_rounds(
        &self,
        mut params: CallToolRequestParams,
        max_rounds: usize,
    ) -> Result<CallToolResult, ServiceError> {
        let mut state_only_rounds = 0usize;
        for _round in 0..max_rounds {
            match self.peer.call_tool_once(params.clone()).await? {
                CallToolResponse::Complete(result) => return Ok(result),
                CallToolResponse::InputRequired(result) => {
                    let (input_responses, request_state) = self
                        .prepare_input_required_retry(result, &mut state_only_rounds)
                        .await?;
                    params.input_responses = input_responses;
                    params.request_state = request_state;
                }
                // SEP-2663: this helper does not drive the task polling
                // lifecycle. Callers that declare the tasks extension
                // capability should use `call_tool_once` and poll `tasks/get`.
                CallToolResponse::Task(_) => return Err(ServiceError::UnexpectedResponse),
            }
        }
        Err(ServiceError::InputRequiredRoundsExceeded { max_rounds })
    }

    /// High-level `prompts/get` helper that automatically fulfils SEP-2322
    /// `input_required` rounds through the local [`ClientHandler`](crate::ClientHandler) service.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::InputRequiredRoundsExceeded`] if the peer does
    /// not produce a final [`GetPromptResult`] within the default MRTR round cap.
    /// Other transport, protocol, and local input-handler errors are propagated.
    pub async fn get_prompt(
        &self,
        params: GetPromptRequestParams,
    ) -> Result<GetPromptResult, ServiceError> {
        self.get_prompt_with_mrtr_max_rounds(params, DEFAULT_MRTR_MAX_ROUNDS)
            .await
    }

    /// Same as [`Self::get_prompt`], with an explicit MRTR round cap.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::InputRequiredRoundsExceeded`] once `max_rounds`
    /// `input_required` responses have been driven without receiving a final
    /// [`GetPromptResult`]. Other transport, protocol, and local input-handler
    /// errors are propagated.
    pub async fn get_prompt_with_mrtr_max_rounds(
        &self,
        mut params: GetPromptRequestParams,
        max_rounds: usize,
    ) -> Result<GetPromptResult, ServiceError> {
        let mut state_only_rounds = 0usize;
        for _round in 0..max_rounds {
            match self.peer.get_prompt_once(params.clone()).await? {
                GetPromptResponse::Complete(result) => return Ok(result),
                GetPromptResponse::InputRequired(result) => {
                    let (input_responses, request_state) = self
                        .prepare_input_required_retry(result, &mut state_only_rounds)
                        .await?;
                    params.input_responses = input_responses;
                    params.request_state = request_state;
                }
            }
        }
        Err(ServiceError::InputRequiredRoundsExceeded { max_rounds })
    }

    /// High-level `resources/read` helper that automatically fulfils SEP-2322
    /// `input_required` rounds through the local [`ClientHandler`](crate::ClientHandler) service.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::InputRequiredRoundsExceeded`] if the peer does
    /// not produce a final [`ReadResourceResult`] within the default MRTR round
    /// cap. Other transport, protocol, and local input-handler errors are
    /// propagated.
    pub async fn read_resource(
        &self,
        params: ReadResourceRequestParams,
    ) -> Result<ReadResourceResult, ServiceError> {
        self.read_resource_with_mrtr_max_rounds(params, DEFAULT_MRTR_MAX_ROUNDS)
            .await
    }

    /// Same as [`Self::read_resource`], with an explicit MRTR round cap.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::InputRequiredRoundsExceeded`] once `max_rounds`
    /// `input_required` responses have been driven without receiving a final
    /// [`ReadResourceResult`]. Other transport, protocol, and local input-handler
    /// errors are propagated.
    pub async fn read_resource_with_mrtr_max_rounds(
        &self,
        mut params: ReadResourceRequestParams,
        max_rounds: usize,
    ) -> Result<ReadResourceResult, ServiceError> {
        let mut state_only_rounds = 0usize;
        for _round in 0..max_rounds {
            match self.peer.read_resource_once(params.clone()).await? {
                ReadResourceResponse::Complete(result) => return Ok(result),
                ReadResourceResponse::InputRequired(result) => {
                    let (input_responses, request_state) = self
                        .prepare_input_required_retry(result, &mut state_only_rounds)
                        .await?;
                    params.input_responses = input_responses;
                    params.request_state = request_state;
                }
            }
        }
        Err(ServiceError::InputRequiredRoundsExceeded { max_rounds })
    }

    async fn prepare_input_required_retry(
        &self,
        result: InputRequiredResult,
        state_only_rounds: &mut usize,
    ) -> Result<(Option<InputResponses>, Option<String>), ServiceError> {
        let had_input_requests = result
            .input_requests
            .as_ref()
            .is_some_and(|requests| !requests.is_empty());
        if !had_input_requests && result.request_state.is_none() {
            return Err(ServiceError::UnexpectedResponse);
        }

        let responses = self
            .fulfill_input_requests(result.input_requests.unwrap_or_default())
            .await?;
        if had_input_requests {
            *state_only_rounds = 0;
        } else {
            Self::sleep_state_only_round(*state_only_rounds).await;
            *state_only_rounds += 1;
        }

        Ok((
            (!responses.is_empty()).then_some(responses),
            result.request_state,
        ))
    }

    async fn fulfill_input_requests(
        &self,
        requests: crate::model::InputRequests,
    ) -> Result<InputResponses, ServiceError> {
        let responses = futures::future::try_join_all(
            requests
                .into_iter()
                .map(|(key, request)| self.fulfill_input_request(key, request)),
        )
        .await?;
        Ok(responses.into_iter().collect())
    }

    async fn fulfill_input_request(
        &self,
        key: String,
        request: InputRequest,
    ) -> Result<(String, serde_json::Value), ServiceError> {
        let response = match request {
            InputRequest::CreateMessage(request) => {
                let mut request = ServerRequest::CreateMessageRequest(request);
                let context = self.input_request_context(&key, &mut request);
                match self
                    .service
                    .handle_request(request, context)
                    .await
                    .map_err(ServiceError::McpError)?
                {
                    ClientResult::CreateMessageResult(result) => {
                        serde_json::to_value(result).map_err(Self::serde_to_service_error)?
                    }
                    _ => return Err(ServiceError::UnexpectedResponse),
                }
            }
            InputRequest::Elicitation(request) => {
                let mut request = ServerRequest::ElicitRequest(request);
                let context = self.input_request_context(&key, &mut request);
                match self
                    .service
                    .handle_request(request, context)
                    .await
                    .map_err(ServiceError::McpError)?
                {
                    ClientResult::ElicitResult(result) => {
                        serde_json::to_value(result).map_err(Self::serde_to_service_error)?
                    }
                    _ => return Err(ServiceError::UnexpectedResponse),
                }
            }
            InputRequest::ListRoots(request) => {
                let mut request = ServerRequest::ListRootsRequest(request);
                let context = self.input_request_context(&key, &mut request);
                match self
                    .service
                    .handle_request(request, context)
                    .await
                    .map_err(ServiceError::McpError)?
                {
                    ClientResult::ListRootsResult(result) => {
                        serde_json::to_value(result).map_err(Self::serde_to_service_error)?
                    }
                    _ => return Err(ServiceError::UnexpectedResponse),
                }
            }
        };
        Ok((key, response))
    }

    fn input_request_context<T>(&self, key: &str, request: &mut T) -> RequestContext<RoleClient>
    where
        T: GetMeta<Metadata = crate::model::RequestMetaObject> + GetExtensions,
    {
        let mut meta = Default::default();
        let mut extensions = Default::default();
        std::mem::swap(&mut meta, request.get_meta_mut());
        std::mem::swap(&mut extensions, request.extensions_mut());
        RequestContext {
            ct: tokio_util::sync::CancellationToken::new(),
            id: NumberOrString::String(Arc::from(key)),
            peer: self.peer.clone(),
            meta,
            extensions,
        }
    }

    async fn sleep_state_only_round(state_only_rounds: usize) {
        let millis = (50u64.saturating_mul(1_u64 << state_only_rounds.min(3))).min(250);
        tokio::time::sleep(Duration::from_millis(millis)).await;
    }

    fn serde_to_service_error(error: serde_json::Error) -> ServiceError {
        ServiceError::McpError(ErrorData::internal_error(
            format!("failed to serialize MRTR input response: {error}"),
            None,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disconnected_peer() -> Peer<RoleClient> {
        let (peer, receiver) =
            Peer::<RoleClient>::new(Arc::new(AtomicU32RequestIdProvider::default()), None);
        drop(receiver);
        peer
    }

    fn tools_result(ttl_ms: Option<u64>, cache_scope: Option<CacheScope>) -> ListToolsResult {
        let mut result = ListToolsResult::with_all_items(Vec::new());
        result.ttl_ms = ttl_ms;
        result.cache_scope = cache_scope;
        result
    }

    #[tokio::test]
    async fn fresh_cached_page_is_served_without_transport_io() {
        let peer = disconnected_peer();
        let params = None::<PaginatedRequestParams>;
        let key = list_response_cache_key(TOOL_LIST_CACHE_PREFIX, &params);
        let expected = tools_result(Some(5_000), Some(CacheScope::Public));
        peer.cache_response(
            key,
            ServerResult::ListToolsResult(expected.clone()),
            expected.ttl_ms,
            expected.cache_scope,
        )
        .await;

        assert_eq!(peer.list_tools(params).await.unwrap(), expected);
    }

    #[tokio::test]
    async fn expired_entry_falls_through_to_the_transport() {
        let peer = disconnected_peer();
        peer.set_response_cache_config(
            ClientCacheConfig::default().with_serve_stale_on_error(false),
        )
        .await;
        let params = None::<PaginatedRequestParams>;
        let key = list_response_cache_key(TOOL_LIST_CACHE_PREFIX, &params);
        peer.cache_response(
            key,
            ServerResult::ListToolsResult(tools_result(Some(1), Some(CacheScope::Public))),
            Some(1),
            Some(CacheScope::Public),
        )
        .await;
        tokio::time::sleep(Duration::from_millis(5)).await;

        assert!(matches!(
            peer.list_tools(params).await,
            Err(ServiceError::TransportClosed)
        ));
    }

    #[tokio::test]
    async fn private_entries_are_isolated_between_authorization_partitions() {
        let peer = disconnected_peer();
        let key = list_response_cache_key(TOOL_LIST_CACHE_PREFIX, &None);

        peer.set_response_cache_config(
            ClientCacheConfig::default().with_private_partition("auth-a"),
        )
        .await;
        peer.cache_response(
            key.clone(),
            ServerResult::ListToolsResult(tools_result(Some(5_000), Some(CacheScope::Private))),
            Some(5_000),
            Some(CacheScope::Private),
        )
        .await;
        assert!(peer.cached_response(&key).await.is_some());

        // Switching to a different authorization context must not expose the
        // first partition's private entry.
        peer.set_response_cache_config(
            ClientCacheConfig::default().with_private_partition("auth-b"),
        )
        .await;
        assert!(peer.cached_response(&key).await.is_none());
    }

    #[tokio::test]
    async fn list_change_notification_discards_every_cached_page() {
        let peer = disconnected_peer();
        for cursor in [None, Some("page-a".into()), Some("page-b".into())] {
            let params =
                cursor.map(|cursor| PaginatedRequestParams::default().with_cursor(Some(cursor)));
            let key = list_response_cache_key(TOOL_LIST_CACHE_PREFIX, &params);
            peer.cache_response(
                key,
                ServerResult::ListToolsResult(tools_result(Some(5_000), Some(CacheScope::Public))),
                Some(5_000),
                Some(CacheScope::Public),
            )
            .await;
        }

        peer.invalidate_tool_cache().await;

        for cursor in [None, Some("page-a".into()), Some("page-b".into())] {
            let params =
                cursor.map(|cursor| PaginatedRequestParams::default().with_cursor(Some(cursor)));
            let key = list_response_cache_key(TOOL_LIST_CACHE_PREFIX, &params);
            assert!(peer.cached_response(&key).await.is_none());
        }
    }

    #[tokio::test]
    async fn expired_entry_is_served_when_refetch_fails() {
        let peer = disconnected_peer();
        let params = None::<PaginatedRequestParams>;
        let key = list_response_cache_key(TOOL_LIST_CACHE_PREFIX, &params);
        let expected = tools_result(Some(1), Some(CacheScope::Public));
        peer.cache_response(
            key,
            ServerResult::ListToolsResult(expected.clone()),
            Some(1),
            Some(CacheScope::Public),
        )
        .await;
        tokio::time::sleep(Duration::from_millis(5)).await;

        assert_eq!(peer.list_tools(params).await.unwrap(), expected);
    }

    #[tokio::test]
    async fn discover_serves_a_fresh_cached_response_without_transport_io() {
        let peer = disconnected_peer();
        let meta = RequestMetaObject::default();
        let key = discover_cache_key();
        let expected = DiscoverResult::new(
            vec![ProtocolVersion::default()],
            Default::default(),
            crate::model::Implementation::from_build_env(),
        )
        .with_ttl_ms(5_000)
        .with_cache_scope(CacheScope::Public);
        peer.cache_response(
            key,
            ServerResult::DiscoverResult(expected.clone()),
            Some(5_000),
            Some(CacheScope::Public),
        )
        .await;

        assert_eq!(peer.discover(meta).await.unwrap(), expected);
    }
}
