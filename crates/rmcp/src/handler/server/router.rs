use std::sync::Arc;

use prompt::{IntoPromptRoute, PromptRoute};
use tool::{IntoToolRoute, ToolRoute};

use super::ServerHandler;
use crate::{
    RoleServer, Service,
    model::{ClientNotification, ClientRequest, ListPromptsResult, ListToolsResult, ServerResult},
    service::NotificationContext,
};

pub mod prompt;
pub mod tool;

#[non_exhaustive]
pub struct Router<S> {
    pub tool_router: tool::ToolRouter<S>,
    pub prompt_router: prompt::PromptRouter<S>,
    pub service: Arc<S>,
    peer_slot: Arc<std::sync::OnceLock<crate::service::Peer<RoleServer>>>,
}

impl<S> Router<S>
where
    S: ServerHandler,
{
    pub fn new(service: S) -> Self {
        let (notifier, peer_slot) = tool::ToolRouter::<S>::deferred_peer_notifier();
        let mut tool_router = tool::ToolRouter::new();
        tool_router.set_notifier(notifier);
        Self {
            tool_router,
            prompt_router: prompt::PromptRouter::new(),
            service: Arc::new(service),
            peer_slot,
        }
    }

    pub fn with_tool<R, A>(mut self, route: R) -> Self
    where
        R: IntoToolRoute<S, A>,
    {
        self.tool_router.add_route(route.into_tool_route());
        self
    }

    pub fn with_tools(mut self, routes: impl IntoIterator<Item = ToolRoute<S>>) -> Self {
        for route in routes {
            self.tool_router.add_route(route);
        }
        self
    }

    pub fn with_prompt<R, A: 'static>(mut self, route: R) -> Self
    where
        R: IntoPromptRoute<S, A>,
    {
        self.prompt_router.add_route(route.into_prompt_route());
        self
    }

    pub fn with_prompts(mut self, routes: impl IntoIterator<Item = PromptRoute<S>>) -> Self {
        for route in routes {
            self.prompt_router.add_route(route);
        }
        self
    }
}

impl<S> Service<RoleServer> for Router<S>
where
    S: ServerHandler,
{
    async fn handle_notification(
        &self,
        notification: <RoleServer as crate::service::ServiceRole>::PeerNot,
        context: NotificationContext<RoleServer>,
    ) -> Result<(), crate::ErrorData> {
        if matches!(
            &notification,
            ClientNotification::InitializedNotification(_)
        ) {
            let _ = self.peer_slot.set(context.peer.clone());
        }
        self.service
            .handle_notification(notification, context)
            .await
    }
    async fn handle_request(
        &self,
        request: <RoleServer as crate::service::ServiceRole>::PeerReq,
        context: crate::service::RequestContext<RoleServer>,
    ) -> Result<<RoleServer as crate::service::ServiceRole>::Resp, crate::ErrorData> {
        match request {
            ClientRequest::CallToolRequest(request) => {
                if self
                    .tool_router
                    .map
                    .contains_key(request.params.name.as_ref())
                    || !self.tool_router.transparent_when_not_found
                {
                    let tool_call_context = crate::handler::server::tool::ToolCallContext::new(
                        self.service.as_ref(),
                        request.params,
                        context,
                    );
                    let result = self.tool_router.call(tool_call_context).await?;
                    Ok(ServerResult::CallToolResult(result))
                } else {
                    self.service
                        .handle_request(ClientRequest::CallToolRequest(request), context)
                        .await
                }
            }
            ClientRequest::ListToolsRequest(_) => {
                let tools = self.tool_router.list_all();
                Ok(ServerResult::ListToolsResult(ListToolsResult {
                    tools,
                    ..Default::default()
                }))
            }
            ClientRequest::GetPromptRequest(request) => {
                if self.prompt_router.has_route(request.params.name.as_ref()) {
                    let prompt_context = crate::handler::server::prompt::PromptContext::new(
                        self.service.as_ref(),
                        request.params.name,
                        request.params.arguments,
                        context,
                    );
                    let result = self.prompt_router.get_prompt(prompt_context).await?;
                    Ok(ServerResult::GetPromptResult(result))
                } else {
                    self.service
                        .handle_request(ClientRequest::GetPromptRequest(request), context)
                        .await
                }
            }
            ClientRequest::ListPromptsRequest(_) => {
                let prompts = self.prompt_router.list_all();
                Ok(ServerResult::ListPromptsResult(ListPromptsResult {
                    prompts,
                    ..Default::default()
                }))
            }
            rest => self.service.handle_request(rest, context).await,
        }
    }

    fn get_info(&self) -> <RoleServer as crate::service::ServiceRole>::Info {
        let mut info = ServerHandler::get_info(&self.service);
        info.capabilities
            .tools
            .get_or_insert_with(Default::default)
            .list_changed = Some(true);
        info
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{
        model::{CallToolResult, ClientNotification, ServerNotification, Tool},
        service::{AtomicU32RequestIdProvider, Peer, PeerSinkMessage, RequestIdProvider},
    };

    struct DummyHandler;
    impl ServerHandler for DummyHandler {}

    async fn recv_notification(
        rx: &mut tokio::sync::mpsc::Receiver<PeerSinkMessage<RoleServer>>,
    ) -> ServerNotification {
        let msg = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");
        match msg {
            PeerSinkMessage::Notification {
                notification,
                responder,
            } => {
                let _ = responder.send(Ok(()));
                notification
            }
            other => panic!("expected notification, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_router_deferred_notifier_e2e() {
        let mut router = Router::new(DummyHandler).with_tool(tool::ToolRoute::new_dyn(
            Tool::new("my_tool", "test", Arc::new(Default::default())),
            |_ctx| Box::pin(async { Ok(CallToolResult::default()) }),
        ));

        let id_provider: Arc<dyn RequestIdProvider> =
            Arc::new(AtomicU32RequestIdProvider::default());
        let (peer, mut rx) = Peer::<RoleServer>::new(id_provider, None);

        let context = crate::service::NotificationContext {
            peer: peer.clone(),
            meta: Default::default(),
            extensions: Default::default(),
        };
        router
            .handle_notification(
                ClientNotification::InitializedNotification(Default::default()),
                context,
            )
            .await
            .unwrap();

        router.tool_router.disable_route("my_tool");
        assert!(matches!(
            recv_notification(&mut rx).await,
            ServerNotification::ToolListChangedNotification(_)
        ));

        router.tool_router.enable_route("my_tool");
        assert!(matches!(
            recv_notification(&mut rx).await,
            ServerNotification::ToolListChangedNotification(_)
        ));
    }
}
