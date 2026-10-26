//! SEP-2640 skills extension, end to end through `Peer` over a duplex transport.
#![cfg(not(feature = "local"))]
#![cfg(all(feature = "client", feature = "server"))]

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use rmcp::{
    ClientHandler, ErrorData, RoleClient, RoleServer, ServerHandler,
    model::{
        CacheScope, ClientConfig, DIRECTORY_MIME_TYPE, ErrorCode, GetSkillRequestParams,
        GetSkillResult, InitializeResult, ListResourcesResult, ListSkillsResult,
        PaginatedRequestParams, ProtocolVersion, ReadResourceDirectoryRequestParams,
        ReadResourceDirectoryResult, ReadResourceRequestParams, ReadResourceResponse,
        ReadResourceResult, Resource, ResourceContents, ServerCapabilities, ServerConfig, Skill,
        SkillFrontmatter, SkillResource, SkillResources,
    },
    service::{RequestContext, RunningService, ServiceError, serve_directly},
};
use tokio::task::JoinHandle;

const SKILL_MD: &str = "---\nname: pdf-processing\ndescription: Extract, fill, and assemble PDF documents\n---\n\n# PDF processing\n\nChoose the matching template from `templates/`.\n";
const PDF_SKILL_URI: &str = "skill://pdf-processing/SKILL.md";
const INVOICE_URI: &str = "skill://pdf-processing/templates/invoice.md";
const TEMPLATES_DIR_URI: &str = "skill://pdf-processing/templates";
const REGIONAL_DIR_URI: &str = "skill://pdf-processing/templates/regional";
const DYNAMIC_SKILL_URI: &str = "skill://live-metrics/SKILL.md";

fn pdf_skill() -> Skill {
    Skill::new(
        PDF_SKILL_URI,
        SkillFrontmatter::new(
            "pdf-processing",
            "Extract, fill, and assemble PDF documents",
        ),
        SkillResources::Files(vec![
            SkillResource::from_bytes(PDF_SKILL_URI, SKILL_MD.as_bytes()),
            SkillResource::from_bytes(INVOICE_URI, b"# Invoice\n"),
        ]),
    )
}

fn dynamic_skill() -> Skill {
    Skill::new(
        DYNAMIC_SKILL_URI,
        SkillFrontmatter::new("live-metrics", "Metrics regenerated on every read"),
        SkillResources::Dynamic,
    )
}

/// Serves two skills one per page, the `pdf-processing` files, and one
/// directory level. Flags switch on the behaviours the tests probe.
#[derive(Debug, Clone, Default)]
struct SkillServer {
    declare_skills: bool,
    declare_directory_read: bool,
    omit_resources: bool,
    answer_get_with_other_uri: bool,
    directory_calls: Arc<AtomicUsize>,
}

impl SkillServer {
    fn with_directory_read() -> Self {
        Self {
            declare_skills: true,
            declare_directory_read: true,
            ..Default::default()
        }
    }

    fn without_directory_read() -> Self {
        Self {
            declare_skills: true,
            ..Default::default()
        }
    }
}

impl ServerHandler for SkillServer {
    fn get_info(&self) -> ServerConfig {
        let builder = ServerCapabilities::builder().enable_resources();
        let mut capabilities = match (self.declare_skills, self.declare_directory_read) {
            (true, true) => builder.enable_skills_with_directory_read().build(),
            (true, false) => builder.enable_skills().build(),
            (false, _) => builder.build(),
        };
        if self.omit_resources {
            capabilities.resources = None;
        }
        ServerConfig::new(capabilities)
    }

    async fn list_skills(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListSkillsResult, ErrorData> {
        let cursor = request.and_then(|params| params.cursor);
        let mut result = match cursor.as_deref() {
            None => ListSkillsResult::with_all_items(vec![pdf_skill()]),
            Some("page-2") => ListSkillsResult::with_all_items(vec![dynamic_skill()]),
            Some(other) => {
                return Err(ErrorData::invalid_params(
                    format!("unknown cursor {other}"),
                    None,
                ));
            }
        };
        if cursor.is_none() {
            result.next_cursor = Some("page-2".into());
        }
        Ok(result)
    }

    async fn get_skill(
        &self,
        request: GetSkillRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetSkillResult, ErrorData> {
        if self.answer_get_with_other_uri {
            return Ok(GetSkillResult::new(dynamic_skill()));
        }
        match request.uri.as_str() {
            PDF_SKILL_URI => Ok(GetSkillResult::new(pdf_skill())),
            DYNAMIC_SKILL_URI => Ok(GetSkillResult::new(dynamic_skill())),
            uri => Err(ErrorData::invalid_params(
                format!("no skill at {uri}"),
                None,
            )),
        }
    }

    async fn read_resource_directory(
        &self,
        request: ReadResourceDirectoryRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceDirectoryResult, ErrorData> {
        self.directory_calls.fetch_add(1, Ordering::SeqCst);
        if request.uri != TEMPLATES_DIR_URI {
            return Err(ErrorData::invalid_params(
                format!("{} is not a directory", request.uri),
                None,
            ));
        }
        Ok(ListResourcesResult::with_all_items(vec![
            Resource::new(INVOICE_URI, "invoice.md").with_mime_type("text/markdown"),
            Resource::new(REGIONAL_DIR_URI, "regional").with_mime_type(DIRECTORY_MIME_TYPE),
        ]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let text = match request.uri.as_str() {
            PDF_SKILL_URI => SKILL_MD,
            INVOICE_URI => "# Invoice\n",
            uri => {
                return Err(ErrorData::resource_not_found(
                    format!("no resource at {uri}"),
                    None,
                ));
            }
        };
        Ok(ReadResourceResult::new(vec![ResourceContents::text(text, request.uri)]).into())
    }
}

#[derive(Debug, Clone)]
struct VersionedClient {
    protocol_version: ProtocolVersion,
}

impl ClientHandler for VersionedClient {
    fn get_info(&self) -> ClientConfig {
        let mut info = ClientConfig::default();
        info.protocol_version = self.protocol_version.clone();
        info
    }
}

type Client = RunningService<RoleClient, VersionedClient>;
type ServerTask = JoinHandle<anyhow::Result<()>>;

/// Wires the pair up with the version already agreed and the server's
/// capabilities known to the client, as the discover lifecycle leaves them.
fn connect(server: SkillServer, protocol_version: ProtocolVersion) -> (Client, ServerTask) {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let client_handler = VersionedClient {
        protocol_version: protocol_version.clone(),
    };
    let mut server_peer_info = InitializeResult::new(server.get_info().capabilities);
    server_peer_info.protocol_version = protocol_version;

    let server = serve_directly::<RoleServer, _, _, _, _>(
        server,
        server_transport,
        Some(client_handler.get_info()),
    );
    let server_task = tokio::spawn(async move {
        server.waiting().await?;
        anyhow::Ok(())
    });
    let client = serve_directly::<RoleClient, _, _, _, _>(
        client_handler,
        client_transport,
        Some(server_peer_info.into()),
    );
    (client, server_task)
}

async fn disconnect(client: Client, server_task: ServerTask) {
    client.cancel().await.expect("client should cancel");
    server_task.await.expect("server task").expect("server");
}

fn error_code(error: ServiceError) -> ErrorCode {
    match error {
        ServiceError::McpError(error) => error.code,
        other => panic!("expected an MCP error, got {other:?}"),
    }
}

#[tokio::test]
async fn list_all_skills_should_follow_the_cursor_across_pages() {
    let (client, server_task) = connect(
        SkillServer::with_directory_read(),
        ProtocolVersion::V_2026_07_28,
    );

    let skills = client.list_all_skills().await.expect("skills/list");
    disconnect(client, server_task).await;

    let names: Vec<_> = skills.iter().map(Skill::name).collect();
    assert_eq!(names, ["pdf-processing", "live-metrics"]);
}

#[tokio::test]
async fn list_skills_should_get_default_cache_hints_for_2026_07_28() {
    let (client, server_task) = connect(
        SkillServer::with_directory_read(),
        ProtocolVersion::V_2026_07_28,
    );

    let page = client.list_skills(None).await.expect("skills/list");
    disconnect(client, server_task).await;

    assert_eq!(
        (page.ttl_ms, page.cache_scope),
        (Some(0), Some(CacheScope::Private))
    );
}

#[tokio::test]
async fn list_skills_should_omit_result_type_for_legacy_peers() {
    let (client, server_task) = connect(
        SkillServer::with_directory_read(),
        ProtocolVersion::V_2025_11_25,
    );

    let page = client.list_skills(None).await.expect("skills/list");
    disconnect(client, server_task).await;

    assert_eq!(
        (page.result_type, page.ttl_ms, page.cache_scope),
        (None, None, None)
    );
}

#[tokio::test]
async fn get_skill_should_return_the_entry_for_a_served_uri() {
    let (client, server_task) = connect(
        SkillServer::with_directory_read(),
        ProtocolVersion::V_2026_07_28,
    );

    let result = client
        .get_skill(GetSkillRequestParams::new(PDF_SKILL_URI))
        .await
        .expect("skills/get");
    disconnect(client, server_task).await;

    assert_eq!(result.skill, pdf_skill());
}

#[tokio::test]
async fn get_skill_should_pass_the_server_invalid_params_error_through() {
    let (client, server_task) = connect(
        SkillServer::with_directory_read(),
        ProtocolVersion::V_2026_07_28,
    );

    let error = client
        .get_skill(GetSkillRequestParams::new("skill://nowhere/SKILL.md"))
        .await
        .expect_err("unknown skill");
    disconnect(client, server_task).await;

    assert_eq!(error_code(error), ErrorCode::INVALID_PARAMS);
}

#[tokio::test]
async fn get_skill_should_reject_a_uri_that_is_not_a_skill_md_before_the_handler() {
    let (client, server_task) = connect(
        SkillServer::with_directory_read(),
        ProtocolVersion::V_2026_07_28,
    );

    let error = client
        .get_skill(GetSkillRequestParams::new(INVOICE_URI))
        .await
        .expect_err("not a SKILL.md");
    disconnect(client, server_task).await;

    assert_eq!(error_code(error), ErrorCode::INVALID_PARAMS);
}

#[tokio::test]
async fn get_skill_should_fail_with_internal_error_when_the_handler_answers_another_uri() {
    let server = SkillServer {
        answer_get_with_other_uri: true,
        ..SkillServer::with_directory_read()
    };
    let (client, server_task) = connect(server, ProtocolVersion::V_2026_07_28);

    let error = client
        .get_skill(GetSkillRequestParams::new(PDF_SKILL_URI))
        .await
        .expect_err("mismatched uri");
    disconnect(client, server_task).await;

    assert_eq!(error_code(error), ErrorCode::INTERNAL_ERROR);
}

#[tokio::test]
async fn skills_methods_should_be_method_not_found_when_the_extension_is_undeclared() {
    let (client, server_task) = connect(SkillServer::default(), ProtocolVersion::V_2026_07_28);

    let error = client.list_skills(None).await.expect_err("undeclared");
    disconnect(client, server_task).await;

    assert_eq!(error_code(error), ErrorCode::METHOD_NOT_FOUND);
}

#[tokio::test]
async fn skills_methods_should_fail_with_internal_error_without_the_resources_capability() {
    let server = SkillServer {
        omit_resources: true,
        ..SkillServer::with_directory_read()
    };
    let (client, server_task) = connect(server, ProtocolVersion::V_2026_07_28);

    let error = client.list_skills(None).await.expect_err("no resources");
    disconnect(client, server_task).await;

    assert_eq!(error_code(error), ErrorCode::INTERNAL_ERROR);
}

#[tokio::test]
async fn read_resource_directory_should_not_send_when_directory_read_is_undeclared() {
    let server = SkillServer::without_directory_read();
    let calls = server.directory_calls.clone();
    let (client, server_task) = connect(server, ProtocolVersion::V_2026_07_28);

    let error = client
        .read_resource_directory(ReadResourceDirectoryRequestParams::new(TEMPLATES_DIR_URI))
        .await
        .expect_err("undeclared directoryRead");
    disconnect(client, server_task).await;

    assert_eq!(
        (error_code(error), calls.load(Ordering::SeqCst)),
        (ErrorCode::METHOD_NOT_FOUND, 0)
    );
}

#[tokio::test]
async fn read_resource_directory_should_list_children_with_directories_marked() {
    let (client, server_task) = connect(
        SkillServer::with_directory_read(),
        ProtocolVersion::V_2026_07_28,
    );

    let listing = client
        .read_resource_directory(ReadResourceDirectoryRequestParams::new(TEMPLATES_DIR_URI))
        .await
        .expect("resources/directory/read");
    disconnect(client, server_task).await;

    let directories: Vec<_> = listing
        .resources
        .iter()
        .filter(|resource| resource.is_directory())
        .map(|resource| resource.uri.as_str())
        .collect();
    assert_eq!(directories, [REGIONAL_DIR_URI]);
}

#[tokio::test]
async fn read_resource_directory_should_pass_invalid_params_through_for_a_file() {
    let (client, server_task) = connect(
        SkillServer::with_directory_read(),
        ProtocolVersion::V_2026_07_28,
    );

    let error = client
        .read_resource_directory(ReadResourceDirectoryRequestParams::new(INVOICE_URI))
        .await
        .expect_err("not a directory");
    disconnect(client, server_task).await;

    assert_eq!(error_code(error), ErrorCode::INVALID_PARAMS);
}

#[tokio::test]
async fn skill_md_read_through_resources_should_verify_against_the_manifest() {
    let (client, server_task) = connect(
        SkillServer::with_directory_read(),
        ProtocolVersion::V_2026_07_28,
    );

    let manifest = client
        .get_skill(GetSkillRequestParams::new(PDF_SKILL_URI))
        .await
        .expect("skills/get");
    let contents = client
        .read_resource(ReadResourceRequestParams::new(PDF_SKILL_URI))
        .await
        .expect("resources/read");
    disconnect(client, server_task).await;

    let SkillResources::Files(files) = &manifest.skill.resources else {
        panic!("expected a file manifest");
    };
    let entry = files.iter().find(|file| file.uri == PDF_SKILL_URI).unwrap();
    let Some(ResourceContents::TextResourceContents { text, .. }) = contents.contents.first()
    else {
        panic!("expected text contents, got {:?}", contents.contents);
    };
    assert_eq!(entry.verify(text.as_bytes()), Ok(()));
}
