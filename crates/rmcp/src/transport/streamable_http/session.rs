use std::collections::HashMap;

use tokio::sync::mpsc::{Receiver, Sender};

use crate::model::{
    CancelledNotificationParam, ClientJsonRpcMessage, ClientRequest, JsonRpcNotification,
    JsonRpcRequest, Notification, ProgressNotificationParam, ProgressToken, RequestId,
    ServerJsonRpcMessage, ServerNotification, WithMeta,
};

pub const HEADER_SESSION_ID: &str = "Mcp-Session-Id";

type SessionId = String;

struct RequestWise {
    progress_token: Option<ProgressToken>,
    tx: Sender<ServerJsonRpcMessage>,
}

pub struct Session {
    id: SessionId,
    request_router: HashMap<RequestId, RequestWise>,
    // ProgressToken - RequestId map
    // pt_rid_map: HashMap<ProgressToken, RequestId>,
    to_service_tx: Sender<ClientJsonRpcMessage>,
    to_client_common_tx: Sender<ServerJsonRpcMessage>,
}

pub enum SessionError {
    DuplicatedRequestId(RequestId),
    RequestWiseChannelClosed(RequestId),
    CommonChannelClosed,
    TransportClosed,
}

enum OutboundChannel {
    RequestWise { id: RequestId, close: bool },
    Common,
}

impl Session {
    const REQUEST_WISE_CHANNEL_SIZE: usize = 16;
    pub fn session_id(&self) -> &SessionId {
        &self.id
    }
    pub async fn send_to_service(&self, message: ClientJsonRpcMessage) -> Result<(), SessionError> {
        if self.to_service_tx.send(message).await.is_err() {
            return Err(SessionError::TransportClosed);
        }
        Ok(())
    }
    pub async fn establish_request_wise_channel(
        &mut self,
        request: ClientRequest,
        request_id: RequestId,
    ) -> Result<Receiver<ServerJsonRpcMessage>, SessionError> {
        if self.request_router.contains_key(&request_id) {
            return Err(SessionError::DuplicatedRequestId(request_id.clone()));
        };
        let progress_token = request
            .get_meta()
            .and_then(|meta| meta.progress_token.clone());
        let (tx, rx) = tokio::sync::mpsc::channel(Self::REQUEST_WISE_CHANNEL_SIZE);
        self.send_to_service(ClientJsonRpcMessage::Request(JsonRpcRequest {
            request,
            id: request_id.clone(),
            jsonrpc: crate::model::JsonRpcVersion2_0,
        }))
        .await?;
        self.request_router
            .insert(request_id.clone(), RequestWise { progress_token, tx });
        Ok(rx)
    }
    fn resolve_outbound_channel(&self, message: &ServerJsonRpcMessage) -> OutboundChannel {
        match &message {
            ServerJsonRpcMessage::Request(_) => OutboundChannel::Common,
            ServerJsonRpcMessage::Notification(JsonRpcNotification {
                notification:
                    ServerNotification::ProgressNotification(Notification {
                        params: ProgressNotificationParam { progress_token, .. },
                        ..
                    }),
                ..
            }) => {
                let id = self.request_router.iter().find_map(|(id, r)| {
                    (r.progress_token.as_ref() == Some(progress_token)).then_some(id)
                });
                if let Some(id) = id {
                    OutboundChannel::RequestWise {
                        id: id.clone(),
                        close: false,
                    }
                } else {
                    OutboundChannel::Common
                }
            }
            ServerJsonRpcMessage::Notification(JsonRpcNotification {
                notification:
                    ServerNotification::CancelledNotification(Notification {
                        params: CancelledNotificationParam { request_id, .. },
                        ..
                    }),
                ..
            }) => OutboundChannel::RequestWise {
                id: request_id.clone(),
                close: false,
            },
            ServerJsonRpcMessage::Notification(_) => OutboundChannel::Common,
            ServerJsonRpcMessage::Response(json_rpc_response) => OutboundChannel::RequestWise {
                id: json_rpc_response.id.clone(),
                close: false,
            },
            ServerJsonRpcMessage::Error(json_rpc_error) => OutboundChannel::RequestWise {
                id: json_rpc_error.id.clone(),
                close: true,
            },
            ServerJsonRpcMessage::BatchRequest(_) | ServerJsonRpcMessage::BatchResponse(_) => {
                // the server side should never yield a batch request or response now
                unreachable!("server side won't yield batch request or response")
            }
        }
    }
    pub async fn handle_server_message(
        &mut self,
        message: ServerJsonRpcMessage,
    ) -> Result<(), SessionError> {
        let outbound_channel = self.resolve_outbound_channel(&message);
        match outbound_channel {
            OutboundChannel::RequestWise { id, close } => {
                let id = id.clone();
                if let Some(request_wise) = self.request_router.get_mut(&id) {
                    if request_wise.tx.send(message).await.is_err() {
                        return Err(SessionError::RequestWiseChannelClosed(id.clone()));
                    }
                    if close {
                        self.request_router.remove(&id);
                    }
                } else {
                    return Err(SessionError::RequestWiseChannelClosed(id));
                }
            }
            OutboundChannel::Common => {
                if self.to_client_common_tx.send(message).await.is_err() {
                    return Err(SessionError::CommonChannelClosed);
                }
            }
        }
        Ok(())
    }
}
