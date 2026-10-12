use std::{borrow::Cow, sync::Arc};

use prompt::{IntoPromptRoute, PromptRoute};
use skill::IntoSkillRoute;
use tool::{IntoToolRoute, ToolRoute};

use super::ServerHandler;
use crate::{
    RoleServer, Service,
    model::{
        ClientNotification, ClientRequest, ListPromptsResult, ListToolsResult, ProtocolVersion,
        ServerResult,
    },
    service::NotificationContext,
};

pub mod prompt;
pub mod skill;
pub mod tool;

#[non_exhaustive]
pub struct Router<S> {
    pub tool_router: tool::ToolRouter<S>,
    pub prompt_router: prompt::PromptRouter<S>,
    pub skill_router: skill::SkillRouter<S>,
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
            skill_router: skill::SkillRouter::new(),
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

    pub fn with_skill<R, A>(mut self, route: R) -> Self
    where
        R: IntoSkillRoute<S, A>,
    {
        self.skill_router.add_route(route.into_skill_route());
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
                    Ok(ServerResult::from(result))
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
                    Ok(ServerResult::from(result))
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
            ClientRequest::SkillsListRequest(_) => {
                let skills = self.skill_router.list_all();
                Ok(ServerResult::SkillsListResult(
                    crate::model::skills::SkillsListResult {
                        result_type: Some(crate::model::ResultType::COMPLETE),
                        skills: skills.into_iter().collect(),
                        next_cursor: None,
                        ttl_ms: None,
                        cache_scope: None,
                    },
                ))
            }
            ClientRequest::SkillsGetRequest(request) => {
                if let Some(route) = self.skill_router.get_by_uri(&request.params.uri) {
                    let skill_context = crate::handler::server::skill::SkillCallContext::new(
                        self.service.as_ref(),
                        request.params.uri.clone(),
                        context,
                    );
                    let result = (route.call)(skill_context).await?;
                    Ok(ServerResult::SkillsGetResult(
                        crate::model::skills::SkillsGetResult::new(result),
                    ))
                } else {
                    self.service
                        .handle_request(ClientRequest::SkillsGetRequest(request), context)
                        .await
                }
            }
            ClientRequest::ResourcesDirectoryReadRequest(request) => {
                // The router owns the skill catalog, so it is authoritative for
                // any URI that matches a route: answer from the route's declared
                // resources. A route with no static file list yields an empty
                // array, which is the correct answer for a file-less skill.
                if let Some(route) = self.skill_router.get_by_uri(&request.params.uri) {
                    let resources: Vec<crate::model::Resource> = match &route.attr.resources {
                        Some(crate::model::skills::SkillResources::FileList(files)) => files
                            .iter()
                            .map(|f| {
                                let name = f.uri.rsplit('/').next().unwrap_or("").to_string();
                                crate::model::Resource::new(f.uri.clone(), name)
                            })
                            .collect(),
                        _ => Vec::new(),
                    };
                    let mut result =
                        crate::model::skills::ResourcesDirectoryReadResult::new(resources);
                    result.result_type = Some(crate::model::ResultType::COMPLETE);
                    Ok(ServerResult::ResourcesDirectoryReadResult(result))
                } else {
                    // No router-declared skill for this URI — let the inner
                    // service answer (it may have its own directory logic).
                    self.service
                        .handle_request(
                            ClientRequest::ResourcesDirectoryReadRequest(request),
                            context,
                        )
                        .await
                }
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

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        ServerHandler::supported_protocol_versions(&self.service)
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

    /// Dummy skill handler used only to build a `SkillRoute` in tests.
    ///
    /// A **named `fn` item** (not a closure) is required: `CallSkillHandler`
    /// is blanket-implemented for `F: for<'a> Fn(&'a S, SkillCallContext<'a, S>)`,
    /// and a fn item is higher-ranked by construction while a closure that
    /// mentions the context type is not ("implementation of `FnOnce` is not
    /// general enough").
    fn dummy_skill_handler<S>(
        _service: &S,
        _ctx: crate::handler::server::skill::SkillCallContext<S>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<crate::model::skills::SkillEntry, crate::ErrorData>,
                > + Send
                + 'static,
        >,
    > {
        Box::pin(async {
            Ok(crate::model::skills::SkillEntry::new(
                "skill://dummy/SKILL.md",
                serde_json::json!({}),
            ))
        })
    }

    fn test_request_context() -> crate::service::RequestContext<RoleServer> {
        let id_provider: Arc<dyn RequestIdProvider> =
            Arc::new(AtomicU32RequestIdProvider::default());
        let (peer, _rx) = Peer::<RoleServer>::new(id_provider, None);
        crate::service::RequestContext::new(crate::model::NumberOrString::Number(1), peer)
    }

    /// **Fix #1** — `resources/directory/read` through the `Router` must answer
    /// from the matching route's declared file list, not a hardcoded empty vec.
    ///
    /// Goes RED before the fix (hardcoded `resources: vec![]` → len 0) and GREEN
    /// after. A route with a real `FileList` is required: a file-less route
    /// returns an empty array either way and cannot distinguish the bug.
    #[tokio::test]
    async fn test_router_directory_read_delegates_to_file_list_route() {
        use crate::model::{
            ClientRequest,
            skills::{
                ResourcesDirectoryReadRequest, ResourcesDirectoryReadRequestParams, SkillEntry,
                SkillResource, SkillResources,
            },
        };

        let mut entry = SkillEntry::new(
            "skill://alpha/SKILL.md",
            serde_json::json!({"name": "alpha", "description": "Alpha skill"}),
        );
        entry.resources = Some(SkillResources::FileList(vec![
            SkillResource::new("skill://alpha/SKILL.md", "digest-a", 10),
            SkillResource::new("skill://alpha/APPENDIX.md", "digest-b", 20),
        ]));

        let mut router = Router::new(DummyHandler);
        router
            .skill_router
            .add_route(skill::SkillRoute::new(entry, dummy_skill_handler));

        let request =
            ClientRequest::ResourcesDirectoryReadRequest(ResourcesDirectoryReadRequest::new(
                ResourcesDirectoryReadRequestParams::new("skill://alpha"),
            ));

        let result = router
            .handle_request(request, test_request_context())
            .await
            .expect("router must answer resources/directory/read");

        let ServerResult::ResourcesDirectoryReadResult(result) = result else {
            panic!("expected ResourcesDirectoryReadResult, got {result:?}");
        };
        assert_eq!(
            result.resources.len(),
            2,
            "router must delegate the route's declared file list; got {:?}",
            result.resources
        );
        let uris: Vec<&str> = result.resources.iter().map(|r| r.uri.as_str()).collect();
        assert!(
            uris.contains(&"skill://alpha/APPENDIX.md"),
            "delegated list must include the route's files; got {uris:?}"
        );
    }

    /// **Fix #4** — a directory-form URI (`skill://alpha`, no file component)
    /// must resolve to its route, not just the file form (`skill://alpha/SKILL.md`).
    ///
    /// Goes RED before the fix (`uri.starts_with("skill://alpha/")` is false for
    /// the bare directory form) and GREEN after. This is the routing layer
    /// `resources/directory/read` depends on.
    #[tokio::test]
    async fn test_skill_router_matches_directory_form_uri() {
        use crate::model::skills::SkillEntry;

        let entry = SkillEntry::new(
            "skill://alpha/SKILL.md",
            serde_json::json!({"name": "alpha", "description": "Alpha skill"}),
        );
        let mut router = skill::SkillRouter::<DummyHandler>::new();
        router.add_route(skill::SkillRoute::new(entry, dummy_skill_handler));

        assert!(
            router.get_by_uri("skill://alpha/SKILL.md").is_some(),
            "file-form URI must match"
        );
        assert!(
            router.get_by_uri("skill://alpha").is_some(),
            "directory-form URI must match after fix"
        );
        assert!(
            router.get_by_uri("skill://nonexistent").is_none(),
            "non-matching URI must not match"
        );
    }

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
            |_ctx| Box::pin(async { Ok(CallToolResult::default().into()) }),
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
