// Sampling/Roots/Logging are SEP-2577-deprecated; internal references are expected.
#![expect(deprecated)]
use std::{borrow::Cow, sync::Arc};

use crate::{
    error::ErrorData as McpError,
    model::*,
    service::{
        MaybeSendFuture, NotificationContext, RequestContext, RoleServer, Service, ServiceRole,
        SubscriptionContext, negotiate_protocol_version, uses_legacy_lifecycle,
    },
};

pub mod common;
pub mod prompt;
mod resource;
pub mod router;
pub mod tool;
pub mod tool_name_validation;
pub mod wrapper;

/// SEP-2663: gate `tasks/*` methods on the client's declared tasks-extension
/// capability.
///
/// - If the server does not advertise the tasks extension, the methods are
///   simply unimplemented: `-32601` Method not found.
/// - If the server advertises it but the client did not declare it (either in
///   the request's `_meta` per-request capabilities or, for session-mode
///   peers, at `initialize` time), the spec requires `-32021` Missing
///   Required Client Capability with the required capability in `data`.
fn validate_tasks_capability<M: ConstString, H: ServerHandler>(
    handler: &H,
    context: &RequestContext<RoleServer>,
) -> Result<(), McpError> {
    if !handler.get_info().capabilities.supports_tasks() {
        return Err(McpError::method_not_found::<M>());
    }
    let client_declared = context
        .client_capabilities()
        .is_some_and(|caps| caps.supports_tasks());
    if client_declared {
        Ok(())
    } else {
        Err(McpError::missing_required_client_capability(
            ClientCapabilities::builder().enable_tasks().build(),
        ))
    }
}

impl<H: ServerHandler> Service<RoleServer> for H {
    async fn handle_request(
        &self,
        request: <RoleServer as ServiceRole>::PeerReq,
        context: RequestContext<RoleServer>,
    ) -> Result<<RoleServer as ServiceRole>::Resp, McpError> {
        // `context` is moved into the dispatch below, so read the negotiated version first.
        let protocol_version = context.protocol_version();
        let mrtr_supported = protocol_version
            .as_ref()
            .is_some_and(|v| v.as_str() >= ProtocolVersion::V_2026_07_28.as_str());
        let requested_version = context.meta.protocol_version();
        let uses_inline_negotiation = !matches!(&request, ClientRequest::InitializeRequest(_));
        if uses_inline_negotiation && let Some(requested_version) = requested_version.as_ref() {
            let supported_versions = self.supported_protocol_versions();
            if !supported_versions.contains(requested_version) {
                return Err(McpError::unsupported_protocol_version(
                    requested_version.clone(),
                    &supported_versions,
                ));
            }
        }
        // Self-contained metadata is required only when the request itself uses
        // the inline lifecycle: a discover opener, a session that started without
        // `initialize`, or a request that declares 2026-07-28+ in its own _meta.
        // Sessions that negotiated via `initialize` (or `serve_directly`) keep the
        // session model and may omit per-request metadata.
        let requires_request_metadata = uses_inline_negotiation
            && (matches!(&request, ClientRequest::DiscoverRequest(_))
                || context.peer.request_metadata_required()
                || requested_version.as_ref().is_some_and(|version| {
                    version.as_str() >= ProtocolVersion::V_2026_07_28.as_str()
                }));
        if requires_request_metadata {
            // Inline lifecycle requests are defined by the 2026-07-28 protocol.
            // Validate that lifecycle contract even when a request selects an
            // older application protocol version.
            let missing = context
                .meta
                .missing_required_keys(&ProtocolVersion::V_2026_07_28);
            if !missing.is_empty() {
                return Err(McpError::invalid_params(
                    format!(
                        "request _meta is missing or has malformed required fields: {}",
                        missing.join(", ")
                    ),
                    None,
                ));
            }
        }
        let legacy_request =
            uses_legacy_lifecycle(protocol_version.as_ref(), requires_request_metadata);
        let result = match request {
            ClientRequest::InitializeRequest(request) => self
                .initialize(request.params, context)
                .await
                .map(ServerResult::InitializeResult),
            ClientRequest::DiscoverRequest(_request) => self
                .discover(context)
                .await
                .map(ServerResult::DiscoverResult),
            ClientRequest::PingRequest(_request) => {
                if !legacy_request {
                    Err(McpError::method_not_found::<PingRequestMethod>())
                } else {
                    self.ping(context).await.map(ServerResult::empty)
                }
            }
            ClientRequest::CompleteRequest(request) => self
                .complete(request.params, context)
                .await
                .map(ServerResult::CompleteResult),
            ClientRequest::SetLevelRequest(request) => self
                .set_level(request.params, context)
                .await
                .map(ServerResult::empty),
            ClientRequest::GetPromptRequest(request) => self
                .get_prompt(request.params, context)
                .await
                .map(ServerResult::from),
            ClientRequest::ListPromptsRequest(request) => self
                .list_prompts(request.params, context)
                .await
                .map(ServerResult::ListPromptsResult),
            ClientRequest::ListResourcesRequest(request) => self
                .list_resources(request.params, context)
                .await
                .map(ServerResult::ListResourcesResult),
            ClientRequest::ListResourceTemplatesRequest(request) => self
                .list_resource_templates(request.params, context)
                .await
                .map(ServerResult::ListResourceTemplatesResult),
            ClientRequest::ReadResourceRequest(request) => self
                .read_resource(request.params, context)
                .await
                .map(ServerResult::from),
            ClientRequest::SubscriptionsListenRequest(request) => {
                if legacy_request {
                    Err(McpError::method_not_found::<SubscriptionsListenRequestMethod>())
                } else {
                    let requested = request.params.notifications;
                    let Some(candidate) = self.accepted_subscription_filter(&requested) else {
                        return Err(
                            McpError::method_not_found::<SubscriptionsListenRequestMethod>(),
                        );
                    };
                    let advertised = requested.supported_by(&self.get_info().capabilities);
                    let handler_accepted = requested.intersection(&candidate);
                    let accepted = handler_accepted.intersection(&advertised);
                    if accepted != handler_accepted {
                        tracing::debug!(
                            requested_resource_count = requested
                                .resource_subscriptions
                                .as_ref()
                                .map_or(0, Vec::len),
                            accepted_resource_count =
                                accepted.resource_subscriptions.as_ref().map_or(0, Vec::len),
                            "subscription filter reduced to advertised server capabilities"
                        );
                    }
                    let subscription_id = context.id.clone();
                    let subscription =
                        SubscriptionContext::establish(context, requested, accepted).await?;
                    // The integrated draft schema defines a final result for graceful
                    // server teardown; explicit stdio cancellation remains a notification.
                    self.listen(subscription).await.map(|()| {
                        ServerResult::SubscriptionsListenResult(
                            SubscriptionsListenResult::complete(subscription_id),
                        )
                    })
                }
            }
            ClientRequest::SubscribeRequest(request) => {
                if !legacy_request {
                    Err(McpError::method_not_found::<SubscribeRequestMethod>())
                } else {
                    self.subscribe(request.params, context)
                        .await
                        .map(ServerResult::empty)
                }
            }
            ClientRequest::UnsubscribeRequest(request) => {
                if !legacy_request {
                    Err(McpError::method_not_found::<UnsubscribeRequestMethod>())
                } else {
                    self.unsubscribe(request.params, context)
                        .await
                        .map(ServerResult::empty)
                }
            }
            ClientRequest::CallToolRequest(request) => {
                let client_declared_tasks = context
                    .client_capabilities()
                    .is_some_and(|caps| caps.supports_tasks());
                let response = self.call_tool(request.params, context).await?;
                // SEP-2663: the server MUST NOT return CreateTaskResult unless
                // the request declared the tasks extension capability. Guard
                // against handlers that fail to check before materializing a
                // task; such clients cannot parse a task handle.
                if matches!(response, CallToolResponse::Task(_)) && !client_declared_tasks {
                    return Err(McpError::missing_required_client_capability(
                        ClientCapabilities::builder().enable_tasks().build(),
                    ));
                }
                Ok(ServerResult::from(response))
            }
            ClientRequest::ListToolsRequest(request) => self
                .list_tools(request.params, context)
                .await
                .map(ServerResult::ListToolsResult),
            ClientRequest::CustomRequest(request) => self
                .on_custom_request(request, context)
                .await
                .map(ServerResult::CustomResult),
            ClientRequest::GetTaskRequest(request) => {
                validate_tasks_capability::<GetTaskMethod, _>(self, &context)?;
                self.get_task(request.params, context)
                    .await
                    .map(ServerResult::GetTaskResult)
            }
            ClientRequest::UpdateTaskRequest(request) => {
                validate_tasks_capability::<UpdateTaskMethod, _>(self, &context)?;
                self.update_task(request.params, context)
                    .await
                    .map(ServerResult::task_ack)
            }
            ClientRequest::CancelTaskRequest(request) => {
                validate_tasks_capability::<CancelTaskMethod, _>(self, &context)?;
                self.cancel_task(request.params, context)
                    .await
                    .map(ServerResult::task_ack)
            }
        };
        let result = result.and_then(|result| {
            if matches!(result, ServerResult::InputRequiredResult(_)) && !mrtr_supported {
                Err(McpError::invalid_request(
                    "InputRequiredResult requires negotiated protocol version 2026-07-28 or newer",
                    None,
                ))
            } else {
                Ok(result)
            }
        });

        // SEP-2164: peers negotiating 2026-07-28+ get the standard INVALID_PARAMS code for
        // resource-not-found; older peers keep RESOURCE_NOT_FOUND. ISO `YYYY-MM-DD` versions
        // compare lexically the same as chronologically.
        let use_invalid_params =
            protocol_version.is_some_and(|v| v.as_str() >= ProtocolVersion::V_2026_07_28.as_str());
        result.map_err(|mut error| {
            if use_invalid_params && error.code == ErrorCode::RESOURCE_NOT_FOUND {
                error.code = ErrorCode::INVALID_PARAMS;
            }
            error
        })
    }

    async fn handle_notification(
        &self,
        notification: <RoleServer as ServiceRole>::PeerNot,
        context: NotificationContext<RoleServer>,
    ) -> Result<(), McpError> {
        match notification {
            ClientNotification::CancelledNotification(notification) => {
                self.on_cancelled(notification.params, context).await
            }
            ClientNotification::ProgressNotification(notification) => {
                self.on_progress(notification.params, context).await
            }
            ClientNotification::InitializedNotification(_notification) => {
                self.on_initialized(context).await
            }
            ClientNotification::RootsListChangedNotification(_notification) => {
                self.on_roots_list_changed(context).await
            }
            ClientNotification::CustomNotification(notification) => {
                self.on_custom_notification(notification, context).await
            }
        };
        Ok(())
    }

    fn get_info(&self) -> <RoleServer as ServiceRole>::Info {
        self.get_info()
    }
}

macro_rules! server_handler_methods {
    () => {
        fn ping(
            &self,
            context: RequestContext<RoleServer>,
        ) -> impl Future<Output = Result<(), McpError>> + MaybeSendFuture + '_ {
            std::future::ready(Ok(()))
        }
        // handle requests
        fn initialize(
            &self,
            request: InitializeRequestParams,
            context: RequestContext<RoleServer>,
        ) -> impl Future<Output = Result<InitializeResult, McpError>> + MaybeSendFuture + '_ {
            context.peer.set_peer_info(request.clone());
            let mut info = self.get_info();
            info.protocol_version = negotiate_protocol_version(
                &request.protocol_version,
                info.protocol_version,
            );
            std::future::ready(Ok(info))
        }
        /// Return the protocol versions supported by this server.
        fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
            Cow::Borrowed(ProtocolVersion::KNOWN_VERSIONS)
        }
        /// Return this server's discovery information.
        fn discover(
            &self,
            context: RequestContext<RoleServer>,
        ) -> impl Future<Output = Result<DiscoverResult, McpError>> + MaybeSendFuture + '_ {
            std::future::ready(Ok(DiscoverResult::from_server_info(
                self.supported_protocol_versions().into_owned(),
                self.get_info(),
            )))
        }
        fn complete(
            &self,
            request: CompleteRequestParams,
            context: RequestContext<RoleServer>,
        ) -> impl Future<Output = Result<CompleteResult, McpError>> + MaybeSendFuture + '_ {
            std::future::ready(Ok(CompleteResult::default()))
        }
        fn set_level(
            &self,
            request: SetLevelRequestParams,
            context: RequestContext<RoleServer>,
        ) -> impl Future<Output = Result<(), McpError>> + MaybeSendFuture + '_ {
            std::future::ready(Err(McpError::method_not_found::<SetLevelRequestMethod>()))
        }
        fn get_prompt(
            &self,
            request: GetPromptRequestParams,
            context: RequestContext<RoleServer>,
        ) -> impl Future<Output = Result<GetPromptResponse, McpError>> + MaybeSendFuture + '_ {
            std::future::ready(Err(McpError::method_not_found::<GetPromptRequestMethod>()))
        }
        fn list_prompts(
            &self,
            request: Option<PaginatedRequestParams>,
            context: RequestContext<RoleServer>,
        ) -> impl Future<Output = Result<ListPromptsResult, McpError>> + MaybeSendFuture + '_ {
            std::future::ready(Ok(ListPromptsResult::default()))
        }
        fn list_resources(
            &self,
            request: Option<PaginatedRequestParams>,
            context: RequestContext<RoleServer>,
        ) -> impl Future<Output = Result<ListResourcesResult, McpError>> + MaybeSendFuture + '_ {
            std::future::ready(Ok(ListResourcesResult::default()))
        }
        fn list_resource_templates(
            &self,
            request: Option<PaginatedRequestParams>,
            context: RequestContext<RoleServer>,
        ) -> impl Future<Output = Result<ListResourceTemplatesResult, McpError>>
               + MaybeSendFuture
               + '_ {
            std::future::ready(Ok(ListResourceTemplatesResult::default()))
        }
        fn read_resource(
            &self,
            request: ReadResourceRequestParams,
            context: RequestContext<RoleServer>,
        ) -> impl Future<Output = Result<ReadResourceResponse, McpError>> + MaybeSendFuture + '_ {
            std::future::ready(Err(
                McpError::method_not_found::<ReadResourceRequestMethod>(),
            ))
        }
        /// Return the subset of a requested notification filter this server accepts.
        ///
        /// Returning `None` leaves `subscriptions/listen` unimplemented. The SDK
        /// intersects the returned filter with both `requested` and the notification
        /// capabilities advertised by [`Self::get_info`] before acknowledging it.
        /// Categories that were not requested or advertised are always removed.
        fn accepted_subscription_filter(
            &self,
            requested: &SubscriptionFilter,
        ) -> Option<SubscriptionFilter> {
            None
        }
        /// Run one established subscription until it is cancelled or closed gracefully.
        ///
        /// The SDK sends the acknowledgment before invoking this method. Returning
        /// `Ok(())` sends the final [`SubscriptionsListenResult`] defined by the
        /// integrated draft schema, marking graceful server teardown. Explicit
        /// stdio cancellation uses `notifications/cancelled` instead.
        fn listen(
            &self,
            context: SubscriptionContext,
        ) -> impl Future<Output = Result<(), McpError>> + MaybeSendFuture + '_ {
            async move {
                context.cancelled().await;
                Ok(())
            }
        }
        #[deprecated(
            note = "resources/subscribe is legacy-only; implement accepted_subscription_filter and listen for protocol version 2026-07-28"
        )]
        fn subscribe(
            &self,
            request: SubscribeRequestParams,
            context: RequestContext<RoleServer>,
        ) -> impl Future<Output = Result<(), McpError>> + MaybeSendFuture + '_ {
            std::future::ready(Err(McpError::method_not_found::<SubscribeRequestMethod>()))
        }
        #[deprecated(
            note = "resources/unsubscribe is legacy-only; subscriptions/listen is cancelled through its request lifecycle"
        )]
        fn unsubscribe(
            &self,
            request: UnsubscribeRequestParams,
            context: RequestContext<RoleServer>,
        ) -> impl Future<Output = Result<(), McpError>> + MaybeSendFuture + '_ {
            std::future::ready(Err(
                McpError::method_not_found::<UnsubscribeRequestMethod>(),
            ))
        }
        /// Handle a `tools/call` request from a client.
        ///
        /// # Choosing a return value
        ///
        /// MCP distinguishes two failure modes; the API forces you to pick
        /// the right one explicitly because they reach the caller's UI very
        /// differently:
        ///
        /// - `Ok(`[`CallToolResult::error`]`(...))` — the tool ran (or tried
        ///   to) and produced a failure the caller should see. The
        ///   `content` you supply is rendered in the caller's MCP client,
        ///   so the user gets your message. **This is the right return
        ///   value for almost every "the tool didn't work" path** — empty
        ///   results, validation failures the user can fix, downstream
        ///   service unavailability, etc.
        ///
        /// - `Err(`[`McpError`]`)` — a JSON-RPC protocol error. Use this
        ///   only when the request itself is unroutable: unknown tool
        ///   ([`ErrorCode::METHOD_NOT_FOUND`]), malformed request shape that
        ///   cannot be treated as a valid `tools/call`, or a server-internal
        ///   failure that means the server cannot serve any request right now
        ///   ([`ErrorCode::INTERNAL_ERROR`], `-32603`). MCP clients
        ///   typically render protocol errors opaquely; **the caller will
        ///   not see your message** — they see something like "Tool result
        ///   missing due to internal error". If you want the caller to read
        ///   your error, use `Ok(CallToolResult::error(...))`.
        ///
        /// See [`CallToolResult::error`] for a worked example.
        fn call_tool(
            &self,
            request: CallToolRequestParams,
            context: RequestContext<RoleServer>,
        ) -> impl Future<Output = Result<CallToolResponse, McpError>> + MaybeSendFuture + '_ {
            std::future::ready(Err(McpError::method_not_found::<CallToolRequestMethod>()))
        }
        fn list_tools(
            &self,
            request: Option<PaginatedRequestParams>,
            context: RequestContext<RoleServer>,
        ) -> impl Future<Output = Result<ListToolsResult, McpError>> + MaybeSendFuture + '_ {
            std::future::ready(Ok(ListToolsResult::default()))
        }
        /// Get a tool definition by name.
        ///
        /// The default implementation returns `None`, which bypasses validation.
        /// When using `#[tool_handler]`, this method is automatically implemented.
        fn get_tool(&self, _name: &str) -> Option<Tool> {
            None
        }
        fn on_custom_request(
            &self,
            request: CustomRequest,
            context: RequestContext<RoleServer>,
        ) -> impl Future<Output = Result<CustomResult, McpError>> + MaybeSendFuture + '_ {
            let CustomRequest { method, .. } = request;
            let _ = context;
            std::future::ready(Err(McpError::new(
                ErrorCode::METHOD_NOT_FOUND,
                method,
                None,
            )))
        }

        fn on_cancelled(
            &self,
            notification: CancelledNotificationParam,
            context: NotificationContext<RoleServer>,
        ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
            std::future::ready(())
        }
        fn on_progress(
            &self,
            notification: ProgressNotificationParam,
            context: NotificationContext<RoleServer>,
        ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
            std::future::ready(())
        }
        fn on_initialized(
            &self,
            context: NotificationContext<RoleServer>,
        ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
            tracing::info!("client initialized");
            std::future::ready(())
        }
        fn on_roots_list_changed(
            &self,
            context: NotificationContext<RoleServer>,
        ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
            std::future::ready(())
        }
        fn on_custom_notification(
            &self,
            notification: CustomNotification,
            context: NotificationContext<RoleServer>,
        ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
            let _ = (notification, context);
            std::future::ready(())
        }

        fn get_info(&self) -> ServerInfo {
            ServerInfo::default()
        }

        /// SEP-2663 `tasks/get`: return the current [`DetailedTask`] state.
        fn get_task(
            &self,
            request: GetTaskParams,
            context: RequestContext<RoleServer>,
        ) -> impl Future<Output = Result<GetTaskResult, McpError>> + MaybeSendFuture + '_ {
            let _ = (request, context);
            std::future::ready(Err(McpError::method_not_found::<GetTaskMethod>()))
        }

        /// SEP-2663 `tasks/update`: accept responses to outstanding in-task
        /// input requests. Returns an empty acknowledgement on success.
        fn update_task(
            &self,
            request: UpdateTaskParams,
            context: RequestContext<RoleServer>,
        ) -> impl Future<Output = Result<(), McpError>> + MaybeSendFuture + '_ {
            let _ = (request, context);
            std::future::ready(Err(McpError::method_not_found::<UpdateTaskMethod>()))
        }

        /// SEP-2663 `tasks/cancel`: cooperative cancellation. Returns an empty
        /// acknowledgement; the task's observable status may lag.
        fn cancel_task(
            &self,
            request: CancelTaskParams,
            context: RequestContext<RoleServer>,
        ) -> impl Future<Output = Result<(), McpError>> + MaybeSendFuture + '_ {
            let _ = (request, context);
            std::future::ready(Err(McpError::method_not_found::<CancelTaskMethod>()))
        }
    };
}

#[allow(unused_variables)]
#[cfg(not(feature = "local"))]
pub trait ServerHandler: Sized + Send + Sync + 'static {
    server_handler_methods!();
}

#[allow(unused_variables)]
#[cfg(feature = "local")]
pub trait ServerHandler: Sized + 'static {
    server_handler_methods!();
}

macro_rules! impl_server_handler_for_wrapper {
    ($wrapper:ident) => {
        impl<T: ServerHandler> ServerHandler for $wrapper<T> {
            fn ping(
                &self,
                context: RequestContext<RoleServer>,
            ) -> impl Future<Output = Result<(), McpError>> + MaybeSendFuture + '_ {
                (**self).ping(context)
            }

            fn initialize(
                &self,
                request: InitializeRequestParams,
                context: RequestContext<RoleServer>,
            ) -> impl Future<Output = Result<InitializeResult, McpError>> + MaybeSendFuture + '_ {
                (**self).initialize(request, context)
            }

            fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
                (**self).supported_protocol_versions()
            }

            fn discover(
                &self,
                context: RequestContext<RoleServer>,
            ) -> impl Future<Output = Result<DiscoverResult, McpError>> + MaybeSendFuture + '_ {
                (**self).discover(context)
            }

            fn complete(
                &self,
                request: CompleteRequestParams,
                context: RequestContext<RoleServer>,
            ) -> impl Future<Output = Result<CompleteResult, McpError>> + MaybeSendFuture + '_ {
                (**self).complete(request, context)
            }

            fn set_level(
                &self,
                request: SetLevelRequestParams,
                context: RequestContext<RoleServer>,
            ) -> impl Future<Output = Result<(), McpError>> + MaybeSendFuture + '_ {
                (**self).set_level(request, context)
            }

            fn get_prompt(
                &self,
                request: GetPromptRequestParams,
                context: RequestContext<RoleServer>,
            ) -> impl Future<Output = Result<GetPromptResponse, McpError>> + MaybeSendFuture + '_ {
                (**self).get_prompt(request, context)
            }

            fn list_prompts(
                &self,
                request: Option<PaginatedRequestParams>,
                context: RequestContext<RoleServer>,
            ) -> impl Future<Output = Result<ListPromptsResult, McpError>> + MaybeSendFuture + '_ {
                (**self).list_prompts(request, context)
            }

            fn list_resources(
                &self,
                request: Option<PaginatedRequestParams>,
                context: RequestContext<RoleServer>,
            ) -> impl Future<Output = Result<ListResourcesResult, McpError>> + MaybeSendFuture + '_ {
                (**self).list_resources(request, context)
            }

            fn list_resource_templates(
                &self,
                request: Option<PaginatedRequestParams>,
                context: RequestContext<RoleServer>,
            ) -> impl Future<Output = Result<ListResourceTemplatesResult, McpError>> + MaybeSendFuture + '_
            {
                (**self).list_resource_templates(request, context)
            }

            fn read_resource(
                &self,
                request: ReadResourceRequestParams,
                context: RequestContext<RoleServer>,
            ) -> impl Future<Output = Result<ReadResourceResponse, McpError>> + MaybeSendFuture + '_ {
                (**self).read_resource(request, context)
            }

            fn accepted_subscription_filter(
                &self,
                requested: &SubscriptionFilter,
            ) -> Option<SubscriptionFilter> {
                (**self).accepted_subscription_filter(requested)
            }

            fn listen(
                &self,
                context: SubscriptionContext,
            ) -> impl Future<Output = Result<(), McpError>> + MaybeSendFuture + '_ {
                (**self).listen(context)
            }

            fn subscribe(
                &self,
                request: SubscribeRequestParams,
                context: RequestContext<RoleServer>,
            ) -> impl Future<Output = Result<(), McpError>> + MaybeSendFuture + '_ {
                (**self).subscribe(request, context)
            }

            fn unsubscribe(
                &self,
                request: UnsubscribeRequestParams,
                context: RequestContext<RoleServer>,
            ) -> impl Future<Output = Result<(), McpError>> + MaybeSendFuture + '_ {
                (**self).unsubscribe(request, context)
            }

            fn call_tool(
                &self,
                request: CallToolRequestParams,
                context: RequestContext<RoleServer>,
            ) -> impl Future<Output = Result<CallToolResponse, McpError>> + MaybeSendFuture + '_ {
                (**self).call_tool(request, context)
            }

            fn list_tools(
                &self,
                request: Option<PaginatedRequestParams>,
                context: RequestContext<RoleServer>,
            ) -> impl Future<Output = Result<ListToolsResult, McpError>> + MaybeSendFuture + '_ {
                (**self).list_tools(request, context)
            }

            fn get_tool(&self, name: &str) -> Option<Tool> {
                (**self).get_tool(name)
            }

            fn on_custom_request(
                &self,
                request: CustomRequest,
                context: RequestContext<RoleServer>,
            ) -> impl Future<Output = Result<CustomResult, McpError>> + MaybeSendFuture + '_ {
                (**self).on_custom_request(request, context)
            }

            fn on_cancelled(
                &self,
                notification: CancelledNotificationParam,
                context: NotificationContext<RoleServer>,
            ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
                (**self).on_cancelled(notification, context)
            }

            fn on_progress(
                &self,
                notification: ProgressNotificationParam,
                context: NotificationContext<RoleServer>,
            ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
                (**self).on_progress(notification, context)
            }

            fn on_initialized(
                &self,
                context: NotificationContext<RoleServer>,
            ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
                (**self).on_initialized(context)
            }

            fn on_roots_list_changed(
                &self,
                context: NotificationContext<RoleServer>,
            ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
                (**self).on_roots_list_changed(context)
            }

            fn on_custom_notification(
                &self,
                notification: CustomNotification,
                context: NotificationContext<RoleServer>,
            ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
                (**self).on_custom_notification(notification, context)
            }

            fn get_info(&self) -> ServerInfo {
                (**self).get_info()
            }

            fn get_task(
                &self,
                request: GetTaskParams,
                context: RequestContext<RoleServer>,
            ) -> impl Future<Output = Result<GetTaskResult, McpError>> + MaybeSendFuture + '_ {
                (**self).get_task(request, context)
            }

            fn update_task(
                &self,
                request: UpdateTaskParams,
                context: RequestContext<RoleServer>,
            ) -> impl Future<Output = Result<(), McpError>> + MaybeSendFuture + '_ {
                (**self).update_task(request, context)
            }

            fn cancel_task(
                &self,
                request: CancelTaskParams,
                context: RequestContext<RoleServer>,
            ) -> impl Future<Output = Result<(), McpError>> + MaybeSendFuture + '_ {
                (**self).cancel_task(request, context)
            }
        }
    };
}

impl_server_handler_for_wrapper!(Box);
impl_server_handler_for_wrapper!(Arc);
