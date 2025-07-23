use std::borrow::Cow;

use thiserror::Error;

use super::*;
use crate::{
    model::{
        CallToolRequest, CallToolRequestParam, CallToolResult, CancelledNotification,
        CancelledNotificationParam, ClientInfo, ClientJsonRpcMessage, ClientNotification,
        ClientRequest, ClientResult, CompleteRequest, CompleteRequestParam, CompleteResult,
        CreateElicitationRequest, CreateElicitationRequestParam, CreateElicitationResult,
        GetPromptRequest, GetPromptRequestParam, GetPromptResult, InitializeRequest,
        InitializedNotification, JsonRpcResponse, ListPromptsRequest, ListPromptsResult,
        ListResourceTemplatesRequest, ListResourceTemplatesResult, ListResourcesRequest,
        ListResourcesResult, ListToolsRequest, ListToolsResult, PaginatedRequestParam,
        ProgressNotification, ProgressNotificationParam, ReadResourceRequest,
        ReadResourceRequestParam, ReadResourceResult, RequestId, RootsListChangedNotification,
        ServerInfo, ServerJsonRpcMessage, ServerNotification, ServerRequest, ServerResult,
        SetLevelRequest, SetLevelRequestParam, SubscribeRequest, SubscribeRequestParam,
        UnsubscribeRequest, UnsubscribeRequestParam,
    },
    transport::DynamicTransportError,
};

/// It represents the error that may occur when serving the client.
///
/// if you want to handle the error, you can use `serve_client_with_ct` or `serve_client` with `Result<RunningService<RoleClient, S>, ClientError>`
#[derive(Error, Debug)]
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
async fn expect_response<T>(
    transport: &mut T,
    context: &str,
) -> Result<(ServerResult, RequestId), ClientInitializeError>
where
    T: Transport<RoleClient>,
{
    let msg = expect_next_message(transport, context).await?;

    match msg {
        ServerJsonRpcMessage::Response(JsonRpcResponse { id, result, .. }) => Ok((result, id)),
        _ => Err(ClientInitializeError::ExpectedInitResponse(Some(msg))),
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
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
    ) -> impl Future<Output = Result<RunningService<RoleClient, Self>, ClientInitializeError>> + Send
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

    let (response, response_id) = expect_response(&mut transport, "initialize response").await?;

    if id != response_id {
        return Err(ClientInitializeError::ConflictInitResponseId(
            id,
            response_id,
        ));
    }

    let ServerResult::InitializeResult(initialize_result) = response else {
        return Err(ClientInitializeError::ExpectedInitResult(Some(response)));
    };

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
    let (peer, peer_rx) = Peer::new(id_provider, Some(initialize_result));
    Ok(serve_inner(service, transport, peer, peer_rx, ct))
}

macro_rules! method {
    (peer_req $method:ident $Req:ident() => $Resp: ident ) => {
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
    (peer_req $method:ident $Req:ident($Param: ident) => $Resp: ident ) => {
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
    (peer_req $method:ident $Req:ident($Param: ident)? => $Resp: ident ) => {
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
    (peer_req $method:ident $Req:ident($Param: ident)) => {
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

    (peer_not $method:ident $Not:ident($Param: ident)) => {
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
    (peer_not $method:ident $Not:ident) => {
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
    method!(peer_req complete CompleteRequest(CompleteRequestParam) => CompleteResult);
    method!(peer_req set_level SetLevelRequest(SetLevelRequestParam));
    method!(peer_req get_prompt GetPromptRequest(GetPromptRequestParam) => GetPromptResult);
    method!(peer_req list_prompts ListPromptsRequest(PaginatedRequestParam)? => ListPromptsResult);
    method!(peer_req list_resources ListResourcesRequest(PaginatedRequestParam)? => ListResourcesResult);
    method!(peer_req list_resource_templates ListResourceTemplatesRequest(PaginatedRequestParam)? => ListResourceTemplatesResult);
    method!(peer_req read_resource ReadResourceRequest(ReadResourceRequestParam) => ReadResourceResult);
    method!(peer_req subscribe SubscribeRequest(SubscribeRequestParam) );
    method!(peer_req unsubscribe UnsubscribeRequest(UnsubscribeRequestParam));
    method!(peer_req call_tool CallToolRequest(CallToolRequestParam) => CallToolResult);
    method!(peer_req list_tools ListToolsRequest(PaginatedRequestParam)? => ListToolsResult);
    method!(peer_req create_elicitation CreateElicitationRequest(CreateElicitationRequestParam) => CreateElicitationResult);

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
                .list_tools(Some(PaginatedRequestParam { cursor }))
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
                .list_prompts(Some(PaginatedRequestParam { cursor }))
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
                .list_resources(Some(PaginatedRequestParam { cursor }))
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
                .list_resource_templates(Some(PaginatedRequestParam { cursor }))
                .await?;
            resource_templates.extend(result.resource_templates);
            cursor = result.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        Ok(resource_templates)
    }

    // =============================================================================
    // ELICITATION CONVENIENCE METHODS
    // =============================================================================

    /// Request a simple yes/no confirmation from the user.
    ///
    /// This is a convenience method for requesting boolean confirmation
    /// from users during tool execution.
    ///
    /// # Arguments
    /// * `message` - The question to ask the user
    ///
    /// # Returns
    /// * `Ok(Some(true))` if user accepted and confirmed
    /// * `Ok(Some(false))` if user accepted but declined
    /// * `Ok(None)` if user declined to answer or cancelled
    ///
    /// # Example
    /// ```rust,no_run
    /// # use rmcp::*;
    /// # async fn example(peer: Peer<RoleClient>) -> Result<(), ServiceError> {
    /// let confirmed = peer.elicit_confirmation("Delete this file?").await?;
    /// match confirmed {
    ///     Some(true) => println!("User confirmed deletion"),
    ///     Some(false) => println!("User declined deletion"),
    ///     None => println!("User cancelled or declined to answer"),
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn elicit_confirmation(
        &self,
        message: impl Into<String>,
    ) -> Result<Option<bool>, ServiceError> {
        use serde_json::json;

        let response = self
            .create_elicitation(CreateElicitationRequestParam {
                message: message.into(),
                requested_schema: json!({
                    "type": "boolean",
                    "description": "User confirmation (true for yes, false for no)"
                })
                .as_object()
                .unwrap()
                .clone(),
            })
            .await?;

        match response.action {
            crate::model::ElicitationAction::Accept => {
                if let Some(value) = response.content {
                    Ok(value.as_bool())
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    /// Request text input from the user.
    ///
    /// This is a convenience method for requesting string input from users.
    ///
    /// # Arguments
    /// * `message` - The prompt message for the user
    /// * `required` - Whether the input is required (cannot be empty)
    ///
    /// # Returns
    /// * `Ok(Some(text))` if user provided input
    /// * `Ok(None)` if user declined or cancelled
    ///
    /// # Example
    /// ```rust,no_run
    /// # use rmcp::*;
    /// # async fn example(peer: Peer<RoleClient>) -> Result<(), ServiceError> {
    /// let name = peer.elicit_text_input("Please enter your name:", false).await?;
    /// if let Some(name) = name {
    ///     println!("Hello, {}!", name);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn elicit_text_input(
        &self,
        message: impl Into<String>,
        required: bool,
    ) -> Result<Option<String>, ServiceError> {
        use serde_json::json;

        let mut schema = json!({
            "type": "string",
            "description": "User text input"
        });

        if required {
            schema["minLength"] = json!(1);
        }

        let response = self
            .create_elicitation(CreateElicitationRequestParam {
                message: message.into(),
                requested_schema: schema.as_object().unwrap().clone(),
            })
            .await?;

        match response.action {
            crate::model::ElicitationAction::Accept => {
                if let Some(value) = response.content {
                    Ok(value.as_str().map(|s| s.to_string()))
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    /// Request the user to choose from multiple options.
    ///
    /// This is a convenience method for presenting users with a list of choices.
    ///
    /// # Arguments
    /// * `message` - The prompt message for the user
    /// * `options` - The available options to choose from
    ///
    /// # Returns
    /// * `Ok(Some(index))` if user selected an option (0-based index)
    /// * `Ok(None)` if user declined or cancelled
    ///
    /// # Example
    /// ```rust,no_run
    /// # use rmcp::*;
    /// # async fn example(peer: Peer<RoleClient>) -> Result<(), ServiceError> {
    /// let options = vec!["Save", "Discard", "Cancel"];
    /// let choice = peer.elicit_choice("What would you like to do?", &options).await?;
    /// match choice {
    ///     Some(0) => println!("User chose to save"),
    ///     Some(1) => println!("User chose to discard"),
    ///     Some(2) => println!("User chose to cancel"),
    ///     _ => println!("User made no choice"),
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn elicit_choice(
        &self,
        message: impl Into<String>,
        options: &[impl AsRef<str>],
    ) -> Result<Option<usize>, ServiceError> {
        use serde_json::json;

        let option_strings: Vec<String> = options.iter().map(|s| s.as_ref().to_string()).collect();

        let response = self
            .create_elicitation(CreateElicitationRequestParam {
                message: message.into(),
                requested_schema: json!({
                    "type": "integer",
                    "minimum": 0,
                    "maximum": option_strings.len() - 1,
                    "description": format!("Choose an option: {}", option_strings.join(", "))
                })
                .as_object()
                .unwrap()
                .clone(),
            })
            .await?;

        match response.action {
            crate::model::ElicitationAction::Accept => {
                if let Some(value) = response.content {
                    if let Some(index) = value.as_u64() {
                        let index = index as usize;
                        if index < options.len() {
                            Ok(Some(index))
                        } else {
                            Ok(None) // Invalid index
                        }
                    } else {
                        Ok(None)
                    }
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    /// Request structured data from the user using a custom JSON schema.
    ///
    /// This is the most flexible elicitation method, allowing you to request
    /// any kind of structured input using JSON Schema validation.
    ///
    /// # Arguments
    /// * `message` - The prompt message for the user
    /// * `schema` - JSON Schema defining the expected data structure
    ///
    /// # Returns
    /// * `Ok(Some(data))` if user provided valid data
    /// * `Ok(None)` if user declined or cancelled
    ///
    /// # Example
    /// ```rust,no_run
    /// # use rmcp::*;
    /// # use serde_json::json;
    /// # async fn example(peer: Peer<RoleClient>) -> Result<(), ServiceError> {
    /// let schema = json!({
    ///     "type": "object",
    ///     "properties": {
    ///         "name": {"type": "string"},
    ///         "email": {"type": "string", "format": "email"},
    ///         "age": {"type": "integer", "minimum": 0}
    ///     },
    ///     "required": ["name", "email"]
    /// });
    ///
    /// let user_data = peer.elicit_structured_input(
    ///     "Please provide your contact information:",
    ///     schema.as_object().unwrap()
    /// ).await?;
    ///
    /// if let Some(data) = user_data {
    ///     println!("Received user data: {}", data);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn elicit_structured_input(
        &self,
        message: impl Into<String>,
        schema: &crate::model::JsonObject,
    ) -> Result<Option<serde_json::Value>, ServiceError> {
        let response = self
            .create_elicitation(CreateElicitationRequestParam {
                message: message.into(),
                requested_schema: schema.clone(),
            })
            .await?;

        match response.action {
            crate::model::ElicitationAction::Accept => Ok(response.content),
            _ => Ok(None),
        }
    }
}
