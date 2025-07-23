use std::borrow::Cow;

use thiserror::Error;

use super::*;
use crate::{
    model::{
        CancelledNotification, CancelledNotificationParam, ClientInfo, ClientJsonRpcMessage,
        ClientNotification, ClientRequest, ClientResult, CreateElicitationRequest,
        CreateElicitationRequestParam, CreateElicitationResult, CreateMessageRequest,
        CreateMessageRequestParam, CreateMessageResult, ErrorData, ListRootsRequest,
        ListRootsResult, LoggingMessageNotification, LoggingMessageNotificationParam,
        ProgressNotification, ProgressNotificationParam, PromptListChangedNotification,
        ProtocolVersion, ResourceListChangedNotification, ResourceUpdatedNotification,
        ResourceUpdatedNotificationParam, ServerInfo, ServerNotification, ServerRequest,
        ServerResult, ToolListChangedNotification,
    },
    transport::DynamicTransportError,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RoleServer;

impl ServiceRole for RoleServer {
    type Req = ServerRequest;
    type Resp = ServerResult;
    type Not = ServerNotification;
    type PeerReq = ClientRequest;
    type PeerResp = ClientResult;
    type PeerNot = ClientNotification;
    type Info = ServerInfo;
    type PeerInfo = ClientInfo;

    type InitializeError = ServerInitializeError;
    const IS_CLIENT: bool = false;
}

/// It represents the error that may occur when serving the server.
///
/// if you want to handle the error, you can use `serve_server_with_ct` or `serve_server` with `Result<RunningService<RoleServer, S>, ServerError>`
#[derive(Error, Debug)]
pub enum ServerInitializeError {
    #[error("expect initialized request, but received: {0:?}")]
    ExpectedInitializeRequest(Option<ClientJsonRpcMessage>),

    #[error("expect initialized notification, but received: {0:?}")]
    ExpectedInitializedNotification(Option<ClientJsonRpcMessage>),

    #[error("connection closed: {0}")]
    ConnectionClosed(String),

    #[error("unexpected initialize result: {0:?}")]
    UnexpectedInitializeResponse(ServerResult),

    #[error("initialize failed: {0}")]
    InitializeFailed(ErrorData),

    #[error("unsupported protocol version: {0}")]
    UnsupportedProtocolVersion(ProtocolVersion),

    #[error("Send message error {error}, when {context}")]
    TransportError {
        error: DynamicTransportError,
        context: Cow<'static, str>,
    },

    #[error("Cancelled")]
    Cancelled,
}

impl ServerInitializeError {
    pub fn transport<T: Transport<RoleServer> + 'static>(
        error: T::Error,
        context: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self::TransportError {
            error: DynamicTransportError::new::<T, _>(error),
            context: context.into(),
        }
    }
}
pub type ClientSink = Peer<RoleServer>;

impl<S: Service<RoleServer>> ServiceExt<RoleServer> for S {
    fn serve_with_ct<T, E, A>(
        self,
        transport: T,
        ct: CancellationToken,
    ) -> impl Future<Output = Result<RunningService<RoleServer, Self>, ServerInitializeError>> + Send
    where
        T: IntoTransport<RoleServer, E, A>,
        E: std::error::Error + Send + Sync + 'static,
        Self: Sized,
    {
        serve_server_with_ct(self, transport, ct)
    }
}

pub async fn serve_server<S, T, E, A>(
    service: S,
    transport: T,
) -> Result<RunningService<RoleServer, S>, ServerInitializeError>
where
    S: Service<RoleServer>,
    T: IntoTransport<RoleServer, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    serve_server_with_ct(service, transport, CancellationToken::new()).await
}

/// Helper function to get the next message from the stream
async fn expect_next_message<T>(
    transport: &mut T,
    context: &str,
) -> Result<ClientJsonRpcMessage, ServerInitializeError>
where
    T: Transport<RoleServer>,
{
    transport
        .receive()
        .await
        .ok_or_else(|| ServerInitializeError::ConnectionClosed(context.to_string()))
}

/// Helper function to expect a request from the stream
async fn expect_request<T>(
    transport: &mut T,
    context: &str,
) -> Result<(ClientRequest, RequestId), ServerInitializeError>
where
    T: Transport<RoleServer>,
{
    let msg = expect_next_message(transport, context).await?;
    let msg_clone = msg.clone();
    msg.into_request()
        .ok_or(ServerInitializeError::ExpectedInitializeRequest(Some(
            msg_clone,
        )))
}

/// Helper function to expect a notification from the stream
async fn expect_notification<T>(
    transport: &mut T,
    context: &str,
) -> Result<ClientNotification, ServerInitializeError>
where
    T: Transport<RoleServer>,
{
    let msg = expect_next_message(transport, context).await?;
    let msg_clone = msg.clone();
    msg.into_notification()
        .ok_or(ServerInitializeError::ExpectedInitializedNotification(
            Some(msg_clone),
        ))
}

pub async fn serve_server_with_ct<S, T, E, A>(
    service: S,
    transport: T,
    ct: CancellationToken,
) -> Result<RunningService<RoleServer, S>, ServerInitializeError>
where
    S: Service<RoleServer>,
    T: IntoTransport<RoleServer, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    tokio::select! {
        result = serve_server_with_ct_inner(service, transport.into_transport(), ct.clone()) => { result }
        _ = ct.cancelled() => {
            Err(ServerInitializeError::Cancelled)
        }
    }
}

async fn serve_server_with_ct_inner<S, T>(
    service: S,
    transport: T,
    ct: CancellationToken,
) -> Result<RunningService<RoleServer, S>, ServerInitializeError>
where
    S: Service<RoleServer>,
    T: Transport<RoleServer> + 'static,
{
    let mut transport = transport.into_transport();
    let id_provider = <Arc<AtomicU32RequestIdProvider>>::default();

    // Get initialize request
    let (request, id) = expect_request(&mut transport, "initialized request").await?;

    let ClientRequest::InitializeRequest(peer_info) = &request else {
        return Err(ServerInitializeError::ExpectedInitializeRequest(Some(
            ClientJsonRpcMessage::request(request, id),
        )));
    };
    let (peer, peer_rx) = Peer::new(id_provider, Some(peer_info.params.clone()));
    let context = RequestContext {
        ct: ct.child_token(),
        id: id.clone(),
        meta: request.get_meta().clone(),
        extensions: request.extensions().clone(),
        peer: peer.clone(),
    };
    // Send initialize response
    let init_response = service.handle_request(request.clone(), context).await;
    let mut init_response = match init_response {
        Ok(ServerResult::InitializeResult(init_response)) => init_response,
        Ok(result) => {
            return Err(ServerInitializeError::UnexpectedInitializeResponse(result));
        }
        Err(e) => {
            transport
                .send(ServerJsonRpcMessage::error(e.clone(), id))
                .await
                .map_err(|error| {
                    ServerInitializeError::transport::<T>(error, "sending error response")
                })?;
            return Err(ServerInitializeError::InitializeFailed(e));
        }
    };
    let peer_protocol_version = peer_info.params.protocol_version.clone();
    let protocol_version = match peer_protocol_version
        .partial_cmp(&init_response.protocol_version)
        .ok_or(ServerInitializeError::UnsupportedProtocolVersion(
            peer_protocol_version,
        ))? {
        std::cmp::Ordering::Less => peer_info.params.protocol_version.clone(),
        _ => init_response.protocol_version,
    };
    init_response.protocol_version = protocol_version;
    transport
        .send(ServerJsonRpcMessage::response(
            ServerResult::InitializeResult(init_response),
            id,
        ))
        .await
        .map_err(|error| {
            ServerInitializeError::transport::<T>(error, "sending initialize response")
        })?;

    // Wait for initialize notification
    let notification = expect_notification(&mut transport, "initialize notification").await?;
    let ClientNotification::InitializedNotification(_) = notification else {
        return Err(ServerInitializeError::ExpectedInitializedNotification(
            Some(ClientJsonRpcMessage::notification(notification)),
        ));
    };
    let context = NotificationContext {
        meta: notification.get_meta().clone(),
        extensions: notification.extensions().clone(),
        peer: peer.clone(),
    };
    let _ = service.handle_notification(notification, context).await;
    // Continue processing service
    Ok(serve_inner(service, transport, peer, peer_rx, ct))
}

macro_rules! method {
    (peer_req $method:ident $Req:ident() => $Resp: ident ) => {
        pub async fn $method(&self) -> Result<$Resp, ServiceError> {
            let result = self
                .send_request(ServerRequest::$Req($Req {
                    method: Default::default(),
                    extensions: Default::default(),
                }))
                .await?;
            match result {
                ClientResult::$Resp(result) => Ok(result),
                _ => Err(ServiceError::UnexpectedResponse),
            }
        }
    };
    (peer_req $method:ident $Req:ident($Param: ident) => $Resp: ident ) => {
        pub async fn $method(&self, params: $Param) -> Result<$Resp, ServiceError> {
            let result = self
                .send_request(ServerRequest::$Req($Req {
                    method: Default::default(),
                    params,
                    extensions: Default::default(),
                }))
                .await?;
            match result {
                ClientResult::$Resp(result) => Ok(result),
                _ => Err(ServiceError::UnexpectedResponse),
            }
        }
    };
    (peer_req $method:ident $Req:ident($Param: ident)) => {
        pub fn $method(
            &self,
            params: $Param,
        ) -> impl Future<Output = Result<(), ServiceError>> + Send + '_ {
            async move {
                let result = self
                    .send_request(ServerRequest::$Req($Req {
                        method: Default::default(),
                        params,
                    }))
                    .await?;
                match result {
                    ClientResult::EmptyResult(_) => Ok(()),
                    _ => Err(ServiceError::UnexpectedResponse),
                }
            }
        }
    };

    (peer_not $method:ident $Not:ident($Param: ident)) => {
        pub async fn $method(&self, params: $Param) -> Result<(), ServiceError> {
            self.send_notification(ServerNotification::$Not($Not {
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
            self.send_notification(ServerNotification::$Not($Not {
                method: Default::default(),
                extensions: Default::default(),
            }))
            .await?;
            Ok(())
        }
    };
}

/// Errors that can occur during typed elicitation operations
#[derive(Error, Debug)]
pub enum ElicitationError {
    /// The elicitation request failed at the service level
    #[error("Service error: {0}")]
    Service(#[from] ServiceError),
    
    /// User declined to provide input or cancelled the request  
    #[error("User declined or cancelled the request")]
    UserDeclined,
    
    /// The response data could not be parsed into the requested type
    #[error("Failed to parse response data: {error}\nReceived data: {data}")]
    ParseError {
        error: serde_json::Error,
        data: serde_json::Value,
    },
    
    /// No response content was provided by the user
    #[error("No response content provided")]
    NoContent,
}

impl Peer<RoleServer> {
    method!(peer_req create_message CreateMessageRequest(CreateMessageRequestParam) => CreateMessageResult);
    method!(peer_req list_roots ListRootsRequest() => ListRootsResult);
    method!(peer_req create_elicitation CreateElicitationRequest(CreateElicitationRequestParam) => CreateElicitationResult);

    method!(peer_not notify_cancelled CancelledNotification(CancelledNotificationParam));
    method!(peer_not notify_progress ProgressNotification(ProgressNotificationParam));
    method!(peer_not notify_logging_message LoggingMessageNotification(LoggingMessageNotificationParam));
    method!(peer_not notify_resource_updated ResourceUpdatedNotification(ResourceUpdatedNotificationParam));
    method!(peer_not notify_resource_list_changed ResourceListChangedNotification);
    method!(peer_not notify_tool_list_changed ToolListChangedNotification);
    method!(peer_not notify_prompt_list_changed PromptListChangedNotification);

    // =============================================================================
    // ELICITATION CONVENIENCE METHODS
    // =============================================================================

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
    /// # async fn example(peer: Peer<RoleServer>) -> Result<(), ServiceError> {
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

    /// Request typed data from the user with automatic schema generation.
    ///
    /// This method automatically generates the JSON schema from the Rust type using `schemars`,
    /// eliminating the need to manually create schemas. The response is automatically parsed
    /// into the requested type.
    ///
    /// **Requires the `elicitation` feature to be enabled.**
    ///
    /// # Type Requirements
    /// The type `T` must implement:
    /// - `schemars::JsonSchema` - for automatic schema generation
    /// - `serde::Deserialize` - for parsing the response
    ///
    /// # Arguments
    /// * `message` - The prompt message for the user
    ///
    /// # Returns
    /// * `Ok(Some(data))` if user provided valid data that matches type T
    /// * `Err(ElicitationError::UserDeclined)` if user declined or cancelled the request
    /// * `Err(ElicitationError::ParseError { .. })` if response data couldn't be parsed into type T
    /// * `Err(ElicitationError::NoContent)` if no response content was provided
    /// * `Err(ElicitationError::Service(_))` if the underlying service call failed
    ///
    /// # Example
    ///
    /// Add to your `Cargo.toml`:
    /// ```toml
    /// [dependencies]
    /// rmcp = { version = "0.3", features = ["elicitation"] }
    /// serde = { version = "1.0", features = ["derive"] }
    /// schemars = "1.0"
    /// ```
    ///
    /// ```rust,no_run
    /// # use rmcp::*;
    /// # use serde::{Deserialize, Serialize};
    /// # use schemars::JsonSchema;
    /// #
    /// #[derive(Debug, Serialize, Deserialize, JsonSchema)]
    /// struct UserProfile {
    ///     #[schemars(description = "Full name")]
    ///     name: String,
    ///     #[schemars(description = "Email address")]
    ///     email: String,
    ///     #[schemars(description = "Age")]
    ///     age: u8,
    /// }
    ///
    /// # async fn example(peer: Peer<RoleServer>) -> Result<(), Box<dyn std::error::Error>> {
    /// match peer.elicit::<UserProfile>("Please enter your profile information").await {
    ///     Ok(Some(profile)) => {
    ///         println!("Name: {}, Email: {}, Age: {}", profile.name, profile.email, profile.age);
    ///     }
    ///     Err(ElicitationError::UserDeclined) => {
    ///         println!("User declined to provide information");
    ///     }
    ///     Err(ElicitationError::ParseError { error, data }) => {
    ///         println!("Failed to parse response: {}\nData: {}", error, data);
    ///     }
    ///     Err(e) => return Err(e.into()),
    /// }
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "schemars")]
    pub async fn elicit<T>(&self, message: impl Into<String>) -> Result<Option<T>, ElicitationError>
    where
        T: schemars::JsonSchema + for<'de> serde::Deserialize<'de>,
    {
        // Generate schema automatically from type
        let schema = crate::handler::server::tool::schema_for_type::<T>();

        let response = self
            .create_elicitation(CreateElicitationRequestParam {
                message: message.into(),
                requested_schema: schema,
            })
            .await?;

        match response.action {
            crate::model::ElicitationAction::Accept => {
                if let Some(value) = response.content {
                    match serde_json::from_value::<T>(value.clone()) {
                        Ok(parsed) => Ok(Some(parsed)),
                        Err(error) => Err(ElicitationError::ParseError { error, data: value }),
                    }
                } else {
                    Err(ElicitationError::NoContent)
                }
            }
            _ => Err(ElicitationError::UserDeclined),
        }
    }
}
