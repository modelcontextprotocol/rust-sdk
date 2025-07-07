use std::sync::Arc;

use tool::{IntoToolRoute, ToolRoute};

use super::ServerHandler;
use crate::{
    RoleServer, Service,
    model::{ClientRequest, ListToolsResult, ServerResult},
    service::NotificationContext,
};

pub mod tool;

pub struct Router<S> {
    pub tool_router: tool::ToolRouter<S>,
    pub service: Arc<S>,
}

impl<S> Router<S>
where
    S: ServerHandler,
{
    pub fn new(service: S) -> Self {
        Self {
            tool_router: tool::ToolRouter::new(),
            service: Arc::new(service),
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
                if self.tool_router.has_route(request.params.name.as_ref())
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
                    next_cursor: None,
                }))
            }
            rest => self.service.handle_request(rest, context).await,
        }
    }

    fn get_info(&self) -> <RoleServer as crate::service::ServiceRole>::Info {
        self.service.get_info()
    }
}
