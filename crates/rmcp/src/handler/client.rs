pub mod progress;
use std::sync::Arc;

use crate::{
    error::ErrorData as McpError,
    model::*,
    service::{
        MaybeSendFuture, NotificationContext, RequestContext, RoleClient, Service, ServiceRole,
    },
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
                .map(Box::new)
                .map(ClientResult::CreateMessageResult),
            ServerRequest::ListRootsRequest(_) => self
                .list_roots(context)
                .await
                .map(ClientResult::ListRootsResult),
            ServerRequest::CreateElicitationRequest(request) => self
                .create_elicitation(request.params, context)
                .await
                .map(ClientResult::CreateElicitationResult),
            ServerRequest::ListTasksRequest(request) => self
                .list_tasks(request.params, context)
                .await
                .map(ClientResult::ListTasksResult),
            ServerRequest::GetTaskInfoRequest(request) => self
                .get_task_info(request.params, context)
                .await
                .map(ClientResult::GetTaskResult),
            ServerRequest::GetTaskResultRequest(request) => self
                .get_task_result(request.params, context)
                .await
                .map(ClientResult::GetTaskPayloadResult),
            ServerRequest::CancelTaskRequest(request) => self
                .cancel_task(request.params, context)
                .await
                .map(ClientResult::CancelTaskResult),
            ServerRequest::CustomRequest(request) => self
                .on_custom_request(request, context)
                .await
                .map(ClientResult::CustomResult),
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
            ServerNotification::ElicitationCompletionNotification(notification) => {
                self.on_url_elicitation_notification_complete(notification.params, context)
                    .await
            }
            ServerNotification::CustomNotification(notification) => {
                self.on_custom_notification(notification, context).await
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
    ) -> impl Future<Output = Result<(), McpError>> + MaybeSendFuture + '_ {
        std::future::ready(Ok(()))
    }

    fn create_message(
        &self,
        params: CreateMessageRequestParams,
        context: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<CreateMessageResult, McpError>> + MaybeSendFuture + '_ {
        std::future::ready(Err(
            McpError::method_not_found::<CreateMessageRequestMethod>(),
        ))
    }

    fn list_roots(
        &self,
        context: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<ListRootsResult, McpError>> + MaybeSendFuture + '_ {
        std::future::ready(Ok(ListRootsResult::default()))
    }

    /// Handle an elicitation request from a server asking for user input.
    ///
    /// This method is called when a server needs interactive input from the user
    /// during tool execution. Implementations should present the message to the user,
    /// collect their input according to the requested schema, and return the result.
    ///
    /// # Arguments
    /// * `request` - The elicitation request with message and schema
    /// * `context` - The request context
    ///
    /// # Returns
    /// The user's response including action (accept/decline/cancel) and optional data
    ///
    /// # Default Behavior
    /// The default implementation automatically declines all elicitation requests.
    /// Real clients should override this to provide user interaction.
    ///
    /// # Example
    /// ```rust,ignore
    /// use rmcp::model::CreateElicitationRequestParam;
    /// use rmcp::{
    ///     model::ErrorData as McpError,
    ///     model::*,
    ///     service::{NotificationContext, RequestContext, RoleClient, Service, ServiceRole},
    /// };
    /// use rmcp::ClientHandler;
    ///
    /// impl ClientHandler for MyClient {
    ///  async fn create_elicitation(
    ///     &self,
    ///     request: CreateElicitationRequestParam,
    ///     context: RequestContext<RoleClient>,
    ///  ) -> Result<CreateElicitationResult, McpError> {
    ///     match request {
    ///         CreateElicitationRequestParam::FormElicitationParam {meta, message, requested_schema,} => {
    ///            // Display message to user and collect input according to requested_schema
    ///           let user_input = get_user_input(message, requested_schema).await?;
    ///          Ok(CreateElicitationResult {
    ///             action: ElicitationAction::Accept,
    ///              content: Some(user_input),
    ///              meta: None,
    ///          })
    ///         }
    ///         CreateElicitationRequestParam::UrlElicitationParam {meta, message, url, elicitation_id,} => {
    ///           // Open URL in browser for user to complete elicitation
    ///           open_url_in_browser(url).await?;
    ///          Ok(CreateElicitationResult {
    ///              action: ElicitationAction::Accept,
    ///             content: None,
    ///             meta: None,
    ///             })
    ///         }
    ///     }
    ///  }
    /// }
    /// ```
    fn create_elicitation(
        &self,
        request: CreateElicitationRequestParams,
        context: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<CreateElicitationResult, McpError>> + MaybeSendFuture + '_
    {
        // Default implementation declines all requests - real clients should override this
        let _ = (request, context);
        std::future::ready(Ok(CreateElicitationResult {
            action: ElicitationAction::Decline,
            content: None,
            meta: None,
        }))
    }

    fn on_custom_request(
        &self,
        request: CustomRequest,
        context: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<CustomResult, McpError>> + MaybeSendFuture + '_ {
        let CustomRequest { method, .. } = request;
        let _ = context;
        std::future::ready(Err(McpError::new(
            ErrorCode::METHOD_NOT_FOUND,
            method,
            None,
        )))
    }

    /// Handle a `tasks/list` request from a server. Only relevant when the
    /// client is also a task *receiver* (e.g. it accepted a task-augmented
    /// `sampling/createMessage` or `elicitation/create` request).
    ///
    /// # Default Behavior
    /// Returns `-32601` (Method not found). Clients that advertise
    /// `capabilities.tasks.list` must override this.
    fn list_tasks(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<ListTasksResult, McpError>> + MaybeSendFuture + '_ {
        let _ = (request, context);
        std::future::ready(Err(McpError::method_not_found::<ListTasksMethod>()))
    }

    /// Handle a `tasks/get` request from a server. Only relevant when the
    /// client is also a task *receiver* (e.g. it accepted a task-augmented
    /// `sampling/createMessage` or `elicitation/create` request).
    ///
    /// # Default Behavior
    /// Returns `-32601` (Method not found). Clients that advertise
    /// `capabilities.tasks.requests.sampling.createMessage` or
    /// `capabilities.tasks.requests.elicitation.create` must override this.
    fn get_task_info(
        &self,
        request: GetTaskInfoParams,
        context: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<GetTaskResult, McpError>> + MaybeSendFuture + '_ {
        let _ = (request, context);
        std::future::ready(Err(McpError::method_not_found::<GetTaskInfoMethod>()))
    }

    /// Handle a `tasks/result` request from a server. Only relevant when
    /// the client is also a task *receiver*.
    ///
    /// # Default Behavior
    /// Returns `-32601` (Method not found).
    fn get_task_result(
        &self,
        request: GetTaskResultParams,
        context: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<GetTaskPayloadResult, McpError>> + MaybeSendFuture + '_ {
        let _ = (request, context);
        std::future::ready(Err(McpError::method_not_found::<GetTaskResultMethod>()))
    }

    /// Handle a `tasks/cancel` request from a server. Only relevant when
    /// the client is also a task *receiver*.
    ///
    /// # Default Behavior
    /// Returns `-32601` (Method not found). Clients that advertise
    /// `capabilities.tasks.cancel` must override this.
    fn cancel_task(
        &self,
        request: CancelTaskParams,
        context: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<CancelTaskResult, McpError>> + MaybeSendFuture + '_ {
        let _ = (request, context);
        std::future::ready(Err(McpError::method_not_found::<CancelTaskMethod>()))
    }

    fn on_cancelled(
        &self,
        params: CancelledNotificationParam,
        context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
        std::future::ready(())
    }
    fn on_progress(
        &self,
        params: ProgressNotificationParam,
        context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
        std::future::ready(())
    }
    fn on_logging_message(
        &self,
        params: LoggingMessageNotificationParam,
        context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
        std::future::ready(())
    }
    fn on_resource_updated(
        &self,
        params: ResourceUpdatedNotificationParam,
        context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
        std::future::ready(())
    }
    fn on_resource_list_changed(
        &self,
        context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
        std::future::ready(())
    }
    fn on_tool_list_changed(
        &self,
        context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
        std::future::ready(())
    }
    fn on_prompt_list_changed(
        &self,
        context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
        std::future::ready(())
    }

    fn on_url_elicitation_notification_complete(
        &self,
        params: ElicitationResponseNotificationParam,
        context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
        std::future::ready(())
    }
    fn on_custom_notification(
        &self,
        notification: CustomNotification,
        context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
        let _ = (notification, context);
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

macro_rules! impl_client_handler_for_wrapper {
    ($wrapper:ident) => {
        impl<T: ClientHandler> ClientHandler for $wrapper<T> {
            fn ping(
                &self,
                context: RequestContext<RoleClient>,
            ) -> impl Future<Output = Result<(), McpError>> + MaybeSendFuture + '_ {
                (**self).ping(context)
            }

            fn create_message(
                &self,
                params: CreateMessageRequestParams,
                context: RequestContext<RoleClient>,
            ) -> impl Future<Output = Result<CreateMessageResult, McpError>> + MaybeSendFuture + '_ {
                (**self).create_message(params, context)
            }

            fn list_roots(
                &self,
                context: RequestContext<RoleClient>,
            ) -> impl Future<Output = Result<ListRootsResult, McpError>> + MaybeSendFuture + '_ {
                (**self).list_roots(context)
            }

            fn create_elicitation(
                &self,
                request: CreateElicitationRequestParams,
                context: RequestContext<RoleClient>,
            ) -> impl Future<Output = Result<CreateElicitationResult, McpError>> + MaybeSendFuture + '_ {
                (**self).create_elicitation(request, context)
            }

            fn on_custom_request(
                &self,
                request: CustomRequest,
                context: RequestContext<RoleClient>,
            ) -> impl Future<Output = Result<CustomResult, McpError>> + MaybeSendFuture + '_ {
                (**self).on_custom_request(request, context)
            }

            fn list_tasks(
                &self,
                request: Option<PaginatedRequestParams>,
                context: RequestContext<RoleClient>,
            ) -> impl Future<Output = Result<ListTasksResult, McpError>> + MaybeSendFuture + '_ {
                (**self).list_tasks(request, context)
            }

            fn get_task_info(
                &self,
                request: GetTaskInfoParams,
                context: RequestContext<RoleClient>,
            ) -> impl Future<Output = Result<GetTaskResult, McpError>> + MaybeSendFuture + '_ {
                (**self).get_task_info(request, context)
            }

            fn get_task_result(
                &self,
                request: GetTaskResultParams,
                context: RequestContext<RoleClient>,
            ) -> impl Future<Output = Result<GetTaskPayloadResult, McpError>> + MaybeSendFuture + '_ {
                (**self).get_task_result(request, context)
            }

            fn cancel_task(
                &self,
                request: CancelTaskParams,
                context: RequestContext<RoleClient>,
            ) -> impl Future<Output = Result<CancelTaskResult, McpError>> + MaybeSendFuture + '_ {
                (**self).cancel_task(request, context)
            }

            fn on_cancelled(
                &self,
                params: CancelledNotificationParam,
                context: NotificationContext<RoleClient>,
            ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
                (**self).on_cancelled(params, context)
            }

            fn on_progress(
                &self,
                params: ProgressNotificationParam,
                context: NotificationContext<RoleClient>,
            ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
                (**self).on_progress(params, context)
            }

            fn on_logging_message(
                &self,
                params: LoggingMessageNotificationParam,
                context: NotificationContext<RoleClient>,
            ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
                (**self).on_logging_message(params, context)
            }

            fn on_resource_updated(
                &self,
                params: ResourceUpdatedNotificationParam,
                context: NotificationContext<RoleClient>,
            ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
                (**self).on_resource_updated(params, context)
            }

            fn on_resource_list_changed(
                &self,
                context: NotificationContext<RoleClient>,
            ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
                (**self).on_resource_list_changed(context)
            }

            fn on_tool_list_changed(
                &self,
                context: NotificationContext<RoleClient>,
            ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
                (**self).on_tool_list_changed(context)
            }

            fn on_prompt_list_changed(
                &self,
                context: NotificationContext<RoleClient>,
            ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
                (**self).on_prompt_list_changed(context)
            }

            fn on_custom_notification(
                &self,
                notification: CustomNotification,
                context: NotificationContext<RoleClient>,
            ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
                (**self).on_custom_notification(notification, context)
            }

            fn get_info(&self) -> ClientInfo {
                (**self).get_info()
            }
        }
    };
}

impl_client_handler_for_wrapper!(Box);
impl_client_handler_for_wrapper!(Arc);
