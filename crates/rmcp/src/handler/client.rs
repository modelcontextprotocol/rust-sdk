use crate::{
    error::Error as McpError,
    model::*,
    service::{NotificationContext, RequestContext, RoleClient, Service, ServiceRole},
};

impl<H: ClientHandler> Service<RoleClient> for H {
    async fn handle_request(
        &self,
        request: <RoleClient as ServiceRole>::PeerReq,
        context: RequestContext<RoleClient>,
    ) -> Result<<RoleClient as ServiceRole>::Resp, McpError> {
        match request {
            ServerRequest::PingRequest(_) => self.ping(context).await.map(ClientResult::empty),
            ServerRequest::CreateMessageRequest(request) => self
                .create_message(request.params, context)
                .await
                .map(ClientResult::CreateMessageResult),
            ServerRequest::ListRootsRequest(_) => self
                .list_roots(context)
                .await
                .map(ClientResult::ListRootsResult),
        }
    }

    async fn handle_notification(
        &self,
        notification: <RoleClient as ServiceRole>::PeerNot,
        context: NotificationContext<RoleClient>,
    ) -> Result<(), McpError> {
        match notification {
            ServerNotification::CancelledNotification(notification) => {
                self.on_cancelled(notification.params, context).await
            }
            ServerNotification::ProgressNotification(notification) => {
                self.on_progress(notification.params, context).await
            }
            ServerNotification::LoggingMessageNotification(notification) => {
                self.on_logging_message(notification.params, context).await
            }
            ServerNotification::ResourceUpdatedNotification(notification) => {
                self.on_resource_updated(notification.params, context).await
            }
            ServerNotification::ResourceListChangedNotification(_notification_no_param) => {
                self.on_resource_list_changed(context).await
            }
            ServerNotification::ToolListChangedNotification(_notification_no_param) => {
                self.on_tool_list_changed(context).await
            }
            ServerNotification::PromptListChangedNotification(_notification_no_param) => {
                self.on_prompt_list_changed(context).await
            }
        };
        Ok(())
    }

    fn get_info(&self) -> <RoleClient as ServiceRole>::Info {
        self.get_info()
    }
}

#[allow(unused_variables)]
pub trait ClientHandler: Sized + Send + Sync + 'static {
    fn ping(
        &self,
        context: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<(), McpError>> + Send + '_ {
        std::future::ready(Ok(()))
    }

    fn create_message(
        &self,
        params: CreateMessageRequestParam,
        context: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<CreateMessageResult, McpError>> + Send + '_ {
        std::future::ready(Err(
            McpError::method_not_found::<CreateMessageRequestMethod>(),
        ))
    }

    fn list_roots(
        &self,
        context: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<ListRootsResult, McpError>> + Send + '_ {
        std::future::ready(Ok(ListRootsResult::default()))
    }

    fn on_cancelled(
        &self,
        params: CancelledNotificationParam,
        context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + Send + '_ {
        std::future::ready(())
    }
    fn on_progress(
        &self,
        params: ProgressNotificationParam,
        context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + Send + '_ {
        std::future::ready(())
    }
    fn on_logging_message(
        &self,
        params: LoggingMessageNotificationParam,
        context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + Send + '_ {
        std::future::ready(())
    }
    fn on_resource_updated(
        &self,
        params: ResourceUpdatedNotificationParam,
        context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + Send + '_ {
        std::future::ready(())
    }
    fn on_resource_list_changed(
        &self,
        context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + Send + '_ {
        std::future::ready(())
    }
    fn on_tool_list_changed(
        &self,
        context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + Send + '_ {
        std::future::ready(())
    }
    fn on_prompt_list_changed(
        &self,
        context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + Send + '_ {
        std::future::ready(())
    }

    fn get_info(&self) -> ClientInfo {
        ClientInfo::default()
    }
}

/// Do nothing, with default client info.
impl ClientHandler for () {}

/// Do nothing, with a specific client info.
impl ClientHandler for ClientInfo {
    fn get_info(&self) -> ClientInfo {
        self.clone()
    }
}
