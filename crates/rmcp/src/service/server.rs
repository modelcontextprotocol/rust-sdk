use futures::{SinkExt, StreamExt};
use thiserror::Error;

use super::*;
use crate::model::{
    CancelledNotification, CancelledNotificationParam, ClientInfo, ClientJsonRpcMessage,
    ClientMessage, ClientNotification, ClientRequest, ClientResult, CreateMessageRequest,
    CreateMessageRequestParam, CreateMessageResult, ListRootsRequest, ListRootsResult,
    LoggingMessageNotification, LoggingMessageNotificationParam, ProgressNotification,
    ProgressNotificationParam, PromptListChangedNotification, ResourceListChangedNotification,
    ResourceUpdatedNotification, ResourceUpdatedNotificationParam, ServerInfo, ServerMessage,
    ServerNotification, ServerRequest, ServerResult, ToolListChangedNotification,
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
    const IS_CLIENT: bool = false;
}

/// It represents the error that may occur when serving the server.
///
/// if you want to handle the error, you can use `serve_server_with_ct` or `serve_server` with `Result<RunningService<RoleServer, S>, ServerError>`
#[derive(Error, Debug)]
pub enum ServerError {
    #[error("expect initialized request, but received: {0:?}")]
    ExpectedInitRequest(Option<ClientMessage>),

    #[error("expect initialized notification, but received: {0:?}")]
    ExpectedInitNotification(Option<ClientMessage>),

    #[error("connection closed: {0}")]
    ConnectionClosed(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type ClientSink = Peer<RoleServer>;

impl<S: Service<RoleServer>> ServiceExt<RoleServer> for S {
    fn serve_with_ct<T, E, A>(
        self,
        transport: T,
        ct: CancellationToken,
    ) -> impl Future<Output = Result<RunningService<RoleServer, Self>, E>> + Send
    where
        T: IntoTransport<RoleServer, E, A>,
        E: std::error::Error + From<std::io::Error> + Send + Sync + 'static,
        Self: Sized,
    {
        serve_server_with_ct(self, transport, ct)
    }
}

pub async fn serve_server<S, T, E, A>(
    service: S,
    transport: T,
) -> Result<RunningService<RoleServer, S>, E>
where
    S: Service<RoleServer>,
    T: IntoTransport<RoleServer, E, A>,
    E: std::error::Error + From<std::io::Error> + Send + Sync + 'static,
{
    serve_server_with_ct(service, transport, CancellationToken::new()).await
}

/// Helper function to get the next message from the stream
async fn expect_next_message<S>(stream: &mut S, context: &str) -> Result<ClientMessage, ServerError>
where
    S: StreamExt<Item = ClientJsonRpcMessage> + Unpin,
{
    Ok(stream
        .next()
        .await
        .ok_or_else(|| ServerError::ConnectionClosed(context.to_string()))?
        .into_message())
}

/// Helper function to expect a request from the stream
async fn expect_request<S>(
    stream: &mut S,
    context: &str,
) -> Result<(ClientRequest, RequestId), ServerError>
where
    S: StreamExt<Item = ClientJsonRpcMessage> + Unpin,
{
    let msg = expect_next_message(stream, context).await?;
    let msg_clone = msg.clone();
    msg.into_request()
        .ok_or(ServerError::ExpectedInitRequest(Some(msg_clone)))
}

/// Helper function to expect a notification from the stream
async fn expect_notification<S>(
    stream: &mut S,
    context: &str,
) -> Result<ClientNotification, ServerError>
where
    S: StreamExt<Item = ClientJsonRpcMessage> + Unpin,
{
    let msg = expect_next_message(stream, context).await?;
    let msg_clone = msg.clone();
    msg.into_notification()
        .ok_or(ServerError::ExpectedInitNotification(Some(msg_clone)))
}

pub async fn serve_server_with_ct<S, T, E, A>(
    service: S,
    transport: T,
    ct: CancellationToken,
) -> Result<RunningService<RoleServer, S>, E>
where
    S: Service<RoleServer>,
    T: IntoTransport<RoleServer, E, A>,
    E: std::error::Error + From<std::io::Error> + Send + Sync + 'static,
{
    let (sink, stream) = transport.into_transport();
    let mut sink = Box::pin(sink);
    let mut stream = Box::pin(stream);
    let id_provider = <Arc<AtomicU32RequestIdProvider>>::default();

    // Convert ServerError to std::io::Error, then to E
    let handle_server_error = |e: ServerError| -> E {
        match e {
            ServerError::Io(io_err) => io_err.into(),
            other => std::io::Error::new(std::io::ErrorKind::Other, format!("{}", other)).into(),
        }
    };

    // Get initialize request
    let (request, id) = expect_request(&mut stream, "initialized request")
        .await
        .map_err(handle_server_error)?;

    let ClientRequest::InitializeRequest(peer_info) = request else {
        return Err(handle_server_error(ServerError::ExpectedInitRequest(Some(
            ClientMessage::Request(request, id),
        ))));
    };

    // Send initialize response
    let init_response = service.get_info();
    sink.send(
        ServerMessage::Response(ServerResult::InitializeResult(init_response), id)
            .into_json_rpc_message(),
    )
    .await?;

    // Wait for initialize notification
    let notification = expect_notification(&mut stream, "initialize notification")
        .await
        .map_err(handle_server_error)?;

    let ClientNotification::InitializedNotification(_) = notification else {
        return Err(handle_server_error(ServerError::ExpectedInitNotification(
            Some(ClientMessage::Notification(notification)),
        )));
    };

    // Continue processing service
    serve_inner(service, (sink, stream), peer_info.params, id_provider, ct).await
}

macro_rules! method {
    (peer_req $method:ident $Req:ident() => $Resp: ident ) => {
        pub async fn $method(&self) -> Result<$Resp, ServiceError> {
            let result = self
                .send_request(ServerRequest::$Req($Req {
                    method: Default::default(),
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
            }))
            .await?;
            Ok(())
        }
    };
    (peer_not $method:ident $Not:ident) => {
        pub async fn $method(&self) -> Result<(), ServiceError> {
            self.send_notification(ServerNotification::$Not($Not {
                method: Default::default(),
            }))
            .await?;
            Ok(())
        }
    };
}

impl Peer<RoleServer> {
    method!(peer_req create_message CreateMessageRequest(CreateMessageRequestParam) => CreateMessageResult);
    method!(peer_req list_roots ListRootsRequest() => ListRootsResult);

    method!(peer_not notify_cancelled CancelledNotification(CancelledNotificationParam));
    method!(peer_not notify_progress ProgressNotification(ProgressNotificationParam));
    method!(peer_not notify_logging_message LoggingMessageNotification(LoggingMessageNotificationParam));
    method!(peer_not notify_resource_updated ResourceUpdatedNotification(ResourceUpdatedNotificationParam));
    method!(peer_not notify_resource_list_changed ResourceListChangedNotification);
    method!(peer_not notify_tool_list_changed ToolListChangedNotification);
    method!(peer_not notify_prompt_list_changed PromptListChangedNotification);
}
