use crate::model::{
    CancelledNotification, CancelledNotificationParam, ClientInfo, ClientNotification,
    ClientRequest, ClientResult, CreateMessageRequest, CreateMessageRequestParam,
    CreateMessageResult, ListRootsRequest, ListRootsResult, LoggingMessageNotification,
    LoggingMessageNotificationParam, ProgressNotification, ProgressNotificationParam,
    PromptListChangedNotification, ResourceListChangedNotification, ResourceUpdatedNotification,
    ResourceUpdatedNotificationParam, ServerInfo, ServerMessage, ServerNotification, ServerRequest,
    ServerResult, ToolListChangedNotification,
};

use super::*;
use futures::{SinkExt, StreamExt};

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

    // service
    let (request, id) = stream
        .next()
        .await
        .ok_or(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "expect initialize request",
        ))?
        .into_message()
        .into_request()
        .ok_or(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "expect initialize request",
        ))?;
    let ClientRequest::InitializeRequest(peer_info) = request else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "expect initialize request",
        )
        .into());
    };
    let init_response = service.get_info();
    sink.send(
        ServerMessage::Response(ServerResult::InitializeResult(init_response), id)
            .into_json_rpc_message(),
    )
    .await?;
    // waiting for notification
    let notification = stream
        .next()
        .await
        .ok_or(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "expect initialize notification",
        ))?
        .into_message()
        .into_notification()
        .ok_or(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "expect initialize notification",
        ))?;
    let ClientNotification::InitializedNotification(_) = notification else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "expect initialize notification",
        )
        .into());
    };
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
