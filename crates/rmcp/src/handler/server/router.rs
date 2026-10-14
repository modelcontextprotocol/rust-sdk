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
                // Spec: the final `<skill-path>` segment of `uri` MUST equal
                // `frontmatter.name`. Entries that violate it are invalid, so
                // they are not served.
                let skills: Vec<_> = self
                    .skill_router
                    .list_all()
                    .into_iter()
                    .filter(|entry| {
                        crate::model::skills::validate_skill_name(&entry.uri, &entry.frontmatter)
                            .is_ok()
                    })
                    .collect();
                let supports_cache_hints = context
                    .protocol_version()
                    .is_some_and(|version| version >= crate::model::ProtocolVersion::V_2026_07_28);
                Ok(ServerResult::SkillsListResult(
                    crate::model::skills::SkillsListResult {
                        result_type: Some(crate::model::ResultType::COMPLETE),
                        skills,
                        next_cursor: None,
                        ttl_ms: supports_cache_hints.then_some(0),
                        cache_scope: supports_cache_hints.then_some("public".to_string()),
                    },
                ))
            }
            ClientRequest::SkillsGetRequest(request) => {
                if let Some(route) = self.skill_router.get_by_uri(&request.params.uri) {
                    // Spec: params.uri MUST be the skill SKILL.md URI. Reject
                    // any other file URI within the skill (e.g., FORMS.md) by
                    // falling through to the default handler, which returns -32602.
                    let directory_form = route
                        .skill_path()
                        .map(|p| request.params.uri == format!("skill://{}", p))
                        .unwrap_or(false);
                    let skill_md_form = crate::model::skills::parse_skill_uri(&request.params.uri)
                        .map(|(_, file)| file == "SKILL.md")
                        .unwrap_or(false);
                    if directory_form || skill_md_form {
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
                    // Spec: the method applies only to directory resources. The
                    // skill root (`skill://<skill-path>`) is a directory; a file
                    // URI such as `skill://<skill-path>/SKILL.md` is not, and
                    // MUST return -32602.
                    let directory_form =
                        format!("skill://{}", route.skill_path().unwrap_or_default());
                    if request.params.uri != directory_form {
                        return Err(crate::ErrorData::invalid_params(
                            format!("not a directory: {}", request.params.uri),
                            None,
                        ));
                    }
                    let resources: Vec<crate::model::Resource> = match &route.attr.resources {
                        Some(crate::model::skills::SkillResources::FileList(files)) => {
                            // Spec, Directory Listing: "The result contains every
                            // direct child of the directory." SKILL.md is a file
                            // directly inside the skill root, so it IS a direct
                            // child and MUST be listed (it is also in the skill's
                            // own `resources` manifest).
                            let root = route
                                .attr
                                .uri
                                .strip_suffix("/SKILL.md")
                                .or_else(|| route.attr.uri.strip_suffix("SKILL.md"))
                                .unwrap_or(&route.attr.uri);
                            let prefix = format!("{}/", root);
                            // Direct children only: a file maps to itself, a
                            // nested path maps to its first segment as a
                            // directory resource. The listing is not recursive.
                            let mut children: std::collections::BTreeMap<String, bool> =
                                std::collections::BTreeMap::new();
                            for file in files {
                                let Some(rest) = file.uri.strip_prefix(&prefix) else {
                                    continue;
                                };
                                if rest.is_empty() {
                                    continue;
                                }
                                match rest.split_once('/') {
                                    Some((head, _)) => {
                                        children.insert(format!("{}/{}", root, head), true);
                                    }
                                    None => {
                                        children.entry(file.uri.clone()).or_insert(false);
                                    }
                                }
                            }
                            children
                                .into_iter()
                                .map(|(uri, is_dir)| {
                                    let name = uri.rsplit('/').next().unwrap_or("").to_string();
                                    let resource = crate::model::Resource::new(uri, name);
                                    if is_dir {
                                        resource.with_mime_type("inode/directory")
                                    } else {
                                        resource
                                    }
                                })
                                .collect()
                        }
                        _ => Vec::new(),
                    };
                    let mut result =
                        crate::model::skills::ResourcesDirectoryReadResult::new(resources);
                    result.result_type = Some(crate::model::ResultType::COMPLETE);
                    Ok(ServerResult::ResourcesDirectoryReadResult(result))
                } else if request.params.uri.starts_with("skill://") {
                    // A `skill://` URI the router does not serve is a request for
                    // a skill this server does not have. Spec line 453: a
                    // non-existent URI MUST return `-32602`, the same code
                    // `resources/read` uses for unknown resources. Delegating to
                    // the inner service would answer `-32601` (method not found),
                    // which misreports a missing skill as an unsupported method.
                    Err(crate::ErrorData::invalid_params(
                        format!("unknown skill URI: {}", request.params.uri),
                        None,
                    ))
                } else {
                    // Not a `skill://` URI — the router has no opinion. Let the
                    // inner service answer (it may have its own directory logic).
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
        // A `Router` that owns skill routes must advertise the SEP-2640 skills
        // extension; otherwise a conformant client never calls `skills/list`.
        // The inner service's own capabilities are preserved — we only add the
        // extension entry.
        if !self.skill_router.map.is_empty() {
            info.capabilities.insert_skills_extension();
            // Spec line 389: "A server declaring this extension MUST also
            // declare the `resources` capability." Without this, a conformant
            // client may reject the server as non-compliant.
            info.capabilities
                .resources
                .get_or_insert_with(Default::default);
        }
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
        // The route declares SKILL.md (its own root) and APPENDIX.md. A directory
        // listing carries every direct child, and SKILL.md is a file directly
        // inside the skill root, so it IS a direct child and is listed.
        assert_eq!(
            result.resources.len(),
            2,
            "router must delegate the route's declared file list, including \
             SKILL.md as a direct child; got {:?}",
            result.resources
        );
        let uris: Vec<&str> = result.resources.iter().map(|r| r.uri.as_str()).collect();
        assert!(
            uris.contains(&"skill://alpha/APPENDIX.md"),
            "delegated list must include the route's files; got {uris:?}"
        );
        assert!(
            uris.contains(&"skill://alpha/SKILL.md"),
            "SKILL.md is a direct child of the skill root and MUST be listed; got {uris:?}"
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

    /// **D1/D8** — a `Router` that owns skill routes must advertise BOTH the
    /// SEP-2640 skills extension AND the `resources` capability.
    ///
    /// Spec line 389: *"A server declaring this extension MUST also declare the
    /// `resources` capability."* Goes RED before the fix (only the extension is
    /// written), GREEN after. The second half is a regression guard: a router
    /// with no skill routes must stay byte-identical to the inner service.
    #[tokio::test]
    async fn test_router_get_info_advertises_skills_extension_and_resources() {
        use crate::model::skills::SkillEntry;
        // No routes -> must not add the resources capability.
        use crate::service::Service;
        let bare = Router::new(DummyHandler);
        let bare_info = bare.get_info();
        assert!(
            bare_info.capabilities.resources.is_none(),
            "a router with no skill routes must not add the resources capability; got {:?}",
            bare_info.capabilities
        );

        // With a route -> extension entry AND the resources capability.
        let entry = SkillEntry::new(
            "skill://alpha/SKILL.md",
            serde_json::json!({"name": "alpha", "description": "Alpha skill"}),
        );
        let mut router = Router::new(DummyHandler);
        router
            .skill_router
            .add_route(skill::SkillRoute::new(entry, dummy_skill_handler));

        let info = router.get_info();
        assert!(
            info.capabilities.resources.is_some(),
            "spec line 389: declaring the skills extension MUST also declare `resources`; got {:?}",
            info.capabilities
        );
        let ext = info
            .capabilities
            .extensions
            .as_ref()
            .and_then(|e| e.get(crate::model::SKILLS_EXTENSION_ID))
            .expect("skills extension must be advertised when skill routes exist");
        assert_eq!(ext["directoryRead"], serde_json::json!(true));
    }

    /// **D2** — a directory read returns every *direct* child: files as ordinary
    /// resources, subdirectories as directory resources (`mimeType:
    /// "inode/directory"`). The listing is not recursive.
    ///
    /// Spec: "The result contains every direct child of the directory: files
    /// with their ordinary resource metadata, subdirectories listed as
    /// directory resources (`mimeType: "inode/directory"`). The listing is not
    /// recursive; clients descend by calling the method again on a child
    /// directory."
    #[tokio::test]
    async fn test_router_directory_read_lists_direct_children_with_directory_mime() {
        use crate::{
            model::{
                ClientRequest,
                skills::{
                    ResourcesDirectoryReadRequest, ResourcesDirectoryReadRequestParams, SkillEntry,
                    SkillResource, SkillResources,
                },
            },
            service::Service,
        };

        let mut entry = SkillEntry::new(
            "skill://pdf-processing/SKILL.md",
            serde_json::json!({"name": "pdf-processing", "description": "PDF skill"}),
        );
        entry.resources = Some(SkillResources::FileList(vec![
            // The skill's own SKILL.md — a direct child of the skill root.
            SkillResource::new("skill://pdf-processing/SKILL.md", "d0", 1),
            // Direct child file.
            SkillResource::new("skill://pdf-processing/README.md", "d1", 2),
            // Two files under templates/ — must collapse to ONE directory child.
            SkillResource::new("skill://pdf-processing/templates/invoice.md", "d2", 3),
            SkillResource::new("skill://pdf-processing/templates/order.md", "d3", 4),
        ]));

        let mut router = Router::new(DummyHandler);
        router
            .skill_router
            .add_route(skill::SkillRoute::new(entry, dummy_skill_handler));

        let request =
            ClientRequest::ResourcesDirectoryReadRequest(ResourcesDirectoryReadRequest::new(
                ResourcesDirectoryReadRequestParams::new("skill://pdf-processing"),
            ));

        let result = router
            .handle_request(request, test_request_context())
            .await
            .expect("router must answer resources/directory/read");
        let ServerResult::ResourcesDirectoryReadResult(result) = result else {
            panic!("expected ResourcesDirectoryReadResult, got {result:?}");
        };

        let uris: Vec<&str> = result.resources.iter().map(|r| r.uri.as_str()).collect();
        assert_eq!(
            uris,
            vec![
                "skill://pdf-processing/README.md",
                "skill://pdf-processing/SKILL.md",
                "skill://pdf-processing/templates"
            ],
            "must list every direct child (SKILL.md included, templates/ collapsed); got {uris:?}"
        );

        // The subdirectory child must be marked as a directory resource.
        let dir_child = result
            .resources
            .iter()
            .find(|r| r.uri == "skill://pdf-processing/templates")
            .expect("templates must appear as a child");
        assert_eq!(
            dir_child.mime_type.as_deref(),
            Some("inode/directory"),
            "subdirectory children must carry mimeType inode/directory"
        );

        // The plain file child must NOT be marked as a directory.
        let file_child = result
            .resources
            .iter()
            .find(|r| r.uri == "skill://pdf-processing/README.md")
            .expect("README.md must appear as a child");
        assert_ne!(
            file_child.mime_type.as_deref(),
            Some("inode/directory"),
            "plain files must not be marked inode/directory"
        );
    }

    /// **D5** — the `frontmatter.name` ↔ URI-final-segment invariant is enforced,
    /// not merely declared.
    ///
    /// Spec: "The final `<skill-path>` segment of the entry's `uri` MUST equal
    /// `frontmatter.name`." `validate_skill_name` existed but had no non-test
    /// caller, so an entry that violated the invariant was served anyway. A
    /// violating entry is invalid, so it must not appear in `skills/list`.
    #[tokio::test]
    async fn test_router_skills_list_filters_name_uri_mismatch() {
        use crate::{
            model::{
                ClientRequest, PaginatedRequestParams, RequestOptionalParam, skills::SkillEntry,
            },
            service::Service,
        };

        // Conforming: uri final segment "alpha" == frontmatter.name "alpha".
        let good = SkillEntry::new(
            "skill://alpha/SKILL.md",
            serde_json::json!({"name": "alpha", "description": "Alpha skill"}),
        );
        // Non-conforming: uri final segment "beta" != frontmatter.name "wrong".
        let bad = SkillEntry::new(
            "skill://beta/SKILL.md",
            serde_json::json!({"name": "wrong", "description": "Mismatched skill"}),
        );

        let mut router = Router::new(DummyHandler);
        router
            .skill_router
            .add_route(skill::SkillRoute::new(good, dummy_skill_handler));
        router
            .skill_router
            .add_route(skill::SkillRoute::new(bad, dummy_skill_handler));

        // Both routes are registered; the invariant is enforced at serve time.
        assert_eq!(
            router.skill_router.list_all().len(),
            2,
            "both routes must be registered for this test to be meaningful"
        );

        let request = ClientRequest::SkillsListRequest(RequestOptionalParam::with_param(
            PaginatedRequestParams::default(),
        ));
        let result = router
            .handle_request(request, test_request_context())
            .await
            .expect("skills/list must succeed");
        let ServerResult::SkillsListResult(result) = result else {
            panic!("expected SkillsListResult, got {result:?}");
        };

        let uris: Vec<&str> = result.skills.iter().map(|s| s.uri.as_str()).collect();
        assert_eq!(
            uris,
            vec!["skill://alpha/SKILL.md"],
            "the entry whose frontmatter.name disagrees with its URI must not be served; got {uris:?}"
        );
    }

    /// **D4** — Router `resources/directory/read` must return `-32602` for
    /// unknown `skill://` URIs, not delegate to the inner service.
    ///
    /// Spec line 453: "A non-existent or non-directory URI MUST return `-32602`."
    /// RED before fix (delegates to inner service → -32601), GREEN after.
    #[tokio::test]
    async fn test_router_directory_read_returns_32602_for_unknown_skill_uri() {
        use crate::{
            model::{
                ClientRequest,
                skills::{
                    ResourcesDirectoryReadRequest, ResourcesDirectoryReadRequestParams, SkillEntry,
                },
            },
            service::Service,
        };

        let entry = SkillEntry::new(
            "skill://alpha/SKILL.md",
            serde_json::json!({"name": "alpha", "description": "Alpha skill"}),
        );
        let mut router = Router::new(DummyHandler);
        router
            .skill_router
            .add_route(skill::SkillRoute::new(entry, dummy_skill_handler));

        let request =
            ClientRequest::ResourcesDirectoryReadRequest(ResourcesDirectoryReadRequest::new(
                ResourcesDirectoryReadRequestParams::new("skill://nonexistent"),
            ));

        let result = router.handle_request(request, test_request_context()).await;
        match result {
            Err(e) => assert_eq!(
                e.code,
                crate::model::ErrorCode(-32602),
                "must return -32602 for unknown skill:// URI, got {:?}",
                e.code
            ),
            Ok(r) => panic!("expected error, got {:?}", r),
        }
    }

    /// **D4 scheme guard** — non-`skill://` URIs must delegate to inner service,
    /// not get -32602. A non-skill URI is not the router's responsibility.
    #[tokio::test]
    async fn test_router_directory_read_delegates_non_skill_uri() {
        use crate::{
            model::{
                ClientRequest,
                skills::{
                    ResourcesDirectoryReadRequest, ResourcesDirectoryReadRequestParams, SkillEntry,
                },
            },
            service::Service,
        };

        let entry = SkillEntry::new(
            "skill://alpha/SKILL.md",
            serde_json::json!({"name": "alpha", "description": "Alpha skill"}),
        );
        let mut router = Router::new(DummyHandler);
        router
            .skill_router
            .add_route(skill::SkillRoute::new(entry, dummy_skill_handler));

        let request =
            ClientRequest::ResourcesDirectoryReadRequest(ResourcesDirectoryReadRequest::new(
                ResourcesDirectoryReadRequestParams::new("file:///etc/passwd"),
            ));

        let result = router.handle_request(request, test_request_context()).await;
        match result {
            Err(e) => assert_eq!(
                e.code,
                crate::model::ErrorCode(-32601),
                "non-skill URI must delegate to inner service (got -32601), got {:?}",
                e.code
            ),
            Ok(r) => panic!("expected delegation error, got {:?}", r),
        }
    }

    /// **D10** — Router `skills/list` must set cache hints when the protocol
    /// version is 2026-07-28+, matching the macro path's behavior.
    ///
    /// Spec line 229: "In protocol versions 2026-07-28 and later, the result
    /// also carries the base protocol's list-caching attributes."
    /// Repo contract (skills.rs:63-64): "REQUIRED on skills/list results in
    /// protocol version 2026-07-28+."
    ///
    /// RED before fix (ttl_ms/cache_scope are None). GREEN after.
    #[tokio::test]
    async fn test_router_skills_list_sets_cache_hints_on_new_protocol() {
        use crate::{model::skills::SkillEntry, service::Service};

        let entry = SkillEntry::new(
            "skill://alpha/SKILL.md",
            serde_json::json!({"name": "alpha", "description": "Alpha skill"}),
        );
        let mut router = Router::new(DummyHandler);
        router
            .skill_router
            .add_route(skill::SkillRoute::new(entry, dummy_skill_handler));

        // Build a RequestContext with protocol version 2026-07-28.
        let (peer, _rx) = crate::service::Peer::new(
            std::sync::Arc::new(crate::service::AtomicU32RequestIdProvider::default()),
            None,
        );
        let mut meta = crate::model::RequestMetaObject::default();
        meta.set_protocol_version(crate::model::ProtocolVersion::V_2026_07_28);
        let context = crate::service::RequestContext {
            ct: tokio_util::sync::CancellationToken::new(),
            id: crate::model::NumberOrString::String("test".to_string().into()),
            meta,
            extensions: crate::model::Extensions::default(),
            peer,
        };

        let request = crate::model::ClientRequest::SkillsListRequest(
            crate::model::RequestOptionalParam::with_param(
                crate::model::PaginatedRequestParams::default(),
            ),
        );

        let result = router
            .handle_request(request, context)
            .await
            .expect("must succeed");
        match result {
            crate::model::ServerResult::SkillsListResult(r) => {
                assert!(
                    r.ttl_ms.is_some(),
                    "ttl_ms must be set on skills/list in protocol 2026-07-28+; got {:?}",
                    r.ttl_ms
                );
                assert!(
                    r.cache_scope.is_some(),
                    "cache_scope must be set on skills/list in protocol 2026-07-28+; got {:?}",
                    r.cache_scope
                );
            }
            other => panic!("expected SkillsListResult, got {:?}", other),
        }
    }

    /// **D10 negative** — Router `skills/list` must NOT set cache hints when the
    /// protocol version is older than 2026-07-28.
    #[tokio::test]
    async fn test_router_skills_list_no_cache_hints_on_old_protocol() {
        use crate::{model::skills::SkillEntry, service::Service};

        let entry = SkillEntry::new(
            "skill://alpha/SKILL.md",
            serde_json::json!({"name": "alpha", "description": "Alpha skill"}),
        );
        let mut router = Router::new(DummyHandler);
        router
            .skill_router
            .add_route(skill::SkillRoute::new(entry, dummy_skill_handler));

        // Default meta = no protocol version set.
        let (peer, _rx) = crate::service::Peer::new(
            std::sync::Arc::new(crate::service::AtomicU32RequestIdProvider::default()),
            None,
        );
        let context = crate::service::RequestContext {
            ct: tokio_util::sync::CancellationToken::new(),
            id: crate::model::NumberOrString::String("test".to_string().into()),
            meta: crate::model::RequestMetaObject::default(),
            extensions: crate::model::Extensions::default(),
            peer,
        };

        let request = crate::model::ClientRequest::SkillsListRequest(
            crate::model::RequestOptionalParam::with_param(
                crate::model::PaginatedRequestParams::default(),
            ),
        );

        let result = router
            .handle_request(request, context)
            .await
            .expect("must succeed");
        match result {
            crate::model::ServerResult::SkillsListResult(r) => {
                assert!(
                    r.ttl_ms.is_none(),
                    "ttl_ms must be None without protocol version; got {:?}",
                    r.ttl_ms
                );
                assert!(
                    r.cache_scope.is_none(),
                    "cache_scope must be None without protocol version; got {:?}",
                    r.cache_scope
                );
            }
            other => panic!("expected SkillsListResult, got {:?}", other),
        }
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
