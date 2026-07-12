// Sampling/Roots/Logging are SEP-2577-deprecated; internal references are expected.
#![expect(deprecated)]
use std::{borrow::Cow, sync::Arc, time::Duration};

use thiserror::Error;

use super::*;
use crate::{
    model::{
        ArgumentInfo, CallToolRequest, CallToolRequestParams, CallToolResponse, CallToolResult,
        CancelledNotification, CancelledNotificationParam, ClientInfo, ClientJsonRpcMessage,
        ClientNotification, ClientRequest, ClientResult, CompleteRequest, CompleteRequestParams,
        CompleteResult, CompletionContext, CompletionInfo, DEFAULT_MRTR_MAX_ROUNDS, ErrorData,
        GetExtensions, GetMeta, GetPromptRequest, GetPromptRequestParams, GetPromptResponse,
        GetPromptResult, InitializeRequest, InitializedNotification, InputRequest,
        InputRequiredResult, InputResponses, JsonRpcResponse, ListPromptsRequest,
        ListPromptsResult, ListResourceTemplatesRequest, ListResourceTemplatesResult,
        ListResourcesRequest, ListResourcesResult, ListToolsRequest, ListToolsResult,
        NumberOrString, PaginatedRequestParams, ProgressNotification, ProgressNotificationParam,
        ReadResourceRequest, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult,
        Reference, RequestId, RootsListChangedNotification, ServerInfo, ServerJsonRpcMessage,
        ServerNotification, ServerRequest, ServerResult, SetLevelRequest, SetLevelRequestParams,
        SubscribeRequest, SubscribeRequestParams, UnsubscribeRequest, UnsubscribeRequestParams,
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
                    meta: Meta::default(),
                    extensions: Extensions::default(),
                };

                if let Some(meta) = logging.extensions.get_mut::<Meta>() {
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
}

pub type ServerSink = Peer<RoleClient>;

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
    serve_client_with_ct(service, transport, Default::default()).await
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
    tokio::select! {
        result = serve_client_with_ct_inner(service, transport.into_transport(), ct.clone()) => { result }
        _ = ct.cancelled() => {
            Err(ClientInitializeError::Cancelled)
        }
    }
}

async fn serve_client_with_ct_inner<S, T>(
    service: S,
    transport: T,
    ct: CancellationToken,
) -> Result<RunningService<RoleClient, S>, ClientInitializeError>
where
    S: Service<RoleClient>,
    T: Transport<RoleClient> + 'static,
{
    let mut transport = transport.into_transport();
    let id_provider = <Arc<AtomicU32RequestIdProvider>>::default();

    // service
    let id = id_provider.next_request_id();
    let init_request = InitializeRequest {
        method: Default::default(),
        params: service.get_info(),
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

    let (peer, peer_rx) = Peer::new(id_provider, None);

    let (response, response_id) = expect_response(
        &mut transport,
        "initialize response",
        &service,
        peer.clone(),
    )
    .await?;

    if id != response_id {
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
    Ok(serve_inner(service, transport, peer, peer_rx, ct))
}

const TOOL_LIST_CACHE_PREFIX: &str = "tools/list:";
const PROMPT_LIST_CACHE_PREFIX: &str = "prompts/list:";
const RESOURCE_LIST_CACHE_PREFIX: &str = "resources/list:";
const RESOURCE_TEMPLATE_LIST_CACHE_PREFIX: &str = "resources/templates/list:";
const RESOURCE_READ_CACHE_PREFIX: &str = "resources/read:";

fn response_cache_key<T: serde::Serialize>(prefix: &str, params: &T) -> Option<String> {
    let value = serde_json::to_value(params).ok()?;

    // SEP-2322 retry rounds may contain user-specific input and request state.
    // Never cache those rounds, even when a server attaches a TTL to the final result.
    let is_mrtr_retry = value
        .get("inputResponses")
        .is_some_and(|value| !value.is_null())
        || value
            .get("requestState")
            .is_some_and(|value| !value.is_null());
    if is_mrtr_retry {
        return None;
    }

    serde_json::to_string(&value)
        .ok()
        .map(|params| format!("{prefix}{params}"))
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
    async fn cache_result(&self, key: Option<String>, ttl_ms: Option<u64>, result: ServerResult) {
        let (Some(key), Some(ttl_ms)) = (key, ttl_ms.filter(|ttl_ms| *ttl_ms > 0)) else {
            return;
        };
        self.cache_response(key, result, Duration::from_millis(ttl_ms))
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

    pub(crate) async fn invalidate_resource_read_cache(&self) {
        self.invalidate_cached_responses(RESOURCE_READ_CACHE_PREFIX)
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
        let cache_key = response_cache_key(RESOURCE_READ_CACHE_PREFIX, &params);
        if let Some(key) = cache_key.as_deref()
            && let Some(ServerResult::ReadResourceResult(result)) = self.cached_response(key).await
        {
            return Ok(ReadResourceResponse::Complete(result));
        }

        let result = self
            .send_request(ClientRequest::ReadResourceRequest(ReadResourceRequest {
                method: Default::default(),
                params,
                extensions: Default::default(),
            }))
            .await?;
        match result {
            ServerResult::ReadResourceResult(result) => {
                self.cache_result(
                    cache_key,
                    result.ttl_ms,
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
    method!(peer_req subscribe SubscribeRequest(SubscribeRequestParams) );
    method!(peer_req unsubscribe UnsubscribeRequest(UnsubscribeRequestParams));
    method!(peer_req call_tool CallToolRequest(CallToolRequestParams) => CallToolResult);

    pub async fn list_prompts(
        &self,
        params: Option<PaginatedRequestParams>,
    ) -> Result<ListPromptsResult, ServiceError> {
        let cache_key = response_cache_key(PROMPT_LIST_CACHE_PREFIX, &params);
        if let Some(key) = cache_key.as_deref()
            && let Some(ServerResult::ListPromptsResult(result)) = self.cached_response(key).await
        {
            return Ok(result);
        }
        let result = self
            .send_request(ClientRequest::ListPromptsRequest(ListPromptsRequest {
                method: Default::default(),
                params,
                extensions: Default::default(),
            }))
            .await?;
        match result {
            ServerResult::ListPromptsResult(result) => {
                self.cache_result(
                    cache_key,
                    result.ttl_ms,
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
        let cache_key = response_cache_key(RESOURCE_LIST_CACHE_PREFIX, &params);
        if let Some(key) = cache_key.as_deref()
            && let Some(ServerResult::ListResourcesResult(result)) = self.cached_response(key).await
        {
            return Ok(result);
        }
        let result = self
            .send_request(ClientRequest::ListResourcesRequest(ListResourcesRequest {
                method: Default::default(),
                params,
                extensions: Default::default(),
            }))
            .await?;
        match result {
            ServerResult::ListResourcesResult(result) => {
                self.cache_result(
                    cache_key,
                    result.ttl_ms,
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
        let cache_key = response_cache_key(RESOURCE_TEMPLATE_LIST_CACHE_PREFIX, &params);
        if let Some(key) = cache_key.as_deref()
            && let Some(ServerResult::ListResourceTemplatesResult(result)) =
                self.cached_response(key).await
        {
            return Ok(result);
        }
        let result = self
            .send_request(ClientRequest::ListResourceTemplatesRequest(
                ListResourceTemplatesRequest {
                    method: Default::default(),
                    params,
                    extensions: Default::default(),
                },
            ))
            .await?;
        match result {
            ServerResult::ListResourceTemplatesResult(result) => {
                self.cache_result(
                    cache_key,
                    result.ttl_ms,
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
        let cache_key = response_cache_key(TOOL_LIST_CACHE_PREFIX, &params);
        if let Some(key) = cache_key.as_deref()
            && let Some(ServerResult::ListToolsResult(result)) = self.cached_response(key).await
        {
            return Ok(result);
        }
        let result = self
            .send_request(ClientRequest::ListToolsRequest(ListToolsRequest {
                method: Default::default(),
                params,
                extensions: Default::default(),
            }))
            .await?;
        match result {
            ServerResult::ListToolsResult(result) => {
                self.cache_result(
                    cache_key,
                    result.ttl_ms,
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
        T: GetMeta + GetExtensions,
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

    #[tokio::test]
    async fn list_tools_returns_a_fresh_cached_page_without_transport_io() {
        let peer = disconnected_peer();
        let params = None::<PaginatedRequestParams>;
        let key = response_cache_key(TOOL_LIST_CACHE_PREFIX, &params).unwrap();
        let expected = ListToolsResult::with_all_items(Vec::new()).with_ttl_ms(5_000);
        peer.cache_response(
            key,
            ServerResult::ListToolsResult(expected.clone()),
            Duration::from_secs(5),
        )
        .await;

        assert_eq!(peer.list_tools(params).await.unwrap(), expected);
    }

    #[tokio::test]
    async fn expired_entries_fall_through_to_the_transport() {
        let peer = disconnected_peer();
        let params = None::<PaginatedRequestParams>;
        let key = response_cache_key(TOOL_LIST_CACHE_PREFIX, &params).unwrap();
        let result = ListToolsResult::with_all_items(Vec::new()).with_ttl_ms(1);
        peer.cache_response(
            key,
            ServerResult::ListToolsResult(result),
            Duration::from_millis(1),
        )
        .await;
        tokio::time::sleep(Duration::from_millis(5)).await;

        assert!(matches!(
            peer.list_tools(params).await,
            Err(ServiceError::TransportClosed)
        ));
    }

    #[tokio::test]
    async fn tool_invalidation_does_not_remove_prompt_pages() {
        let peer = disconnected_peer();
        let params = None::<PaginatedRequestParams>;
        let tool_key = response_cache_key(TOOL_LIST_CACHE_PREFIX, &params).unwrap();
        let prompt_key = response_cache_key(PROMPT_LIST_CACHE_PREFIX, &params).unwrap();
        peer.cache_response(
            tool_key.clone(),
            ServerResult::ListToolsResult(ListToolsResult::with_all_items(Vec::new())),
            Duration::from_secs(5),
        )
        .await;
        peer.cache_response(
            prompt_key.clone(),
            ServerResult::ListPromptsResult(ListPromptsResult::with_all_items(Vec::new())),
            Duration::from_secs(5),
        )
        .await;

        peer.invalidate_tool_cache().await;

        assert!(peer.cached_response(&tool_key).await.is_none());
        assert!(peer.cached_response(&prompt_key).await.is_some());
    }

    #[test]
    fn mrtr_retry_parameters_are_not_cacheable() {
        let params = serde_json::json!({
            "uri": "file:///example.txt",
            "inputResponses": { "approval": true }
        });
        assert!(response_cache_key(RESOURCE_READ_CACHE_PREFIX, &params).is_none());
    }
}
