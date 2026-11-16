//! Stdio server that serves the skills found under a directory (SEP-2640).
//!
//! Every folder below the root that holds a `SKILL.md` becomes a skill; the
//! catalog does the walking, the manifests, the digests, and `resources/read`.
//! The root is the first argument, defaulting to the `skills/` folder next to
//! this crate.
//!
//! Run a matching client with:
//!   cargo run -p mcp-client-examples --example clients_skills_stdio

use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt,
    handler::server::skill_catalog::SkillCatalog,
    model::{
        GetSkillRequestParams, GetSkillResult, ListResourcesResult, ListSkillsResult,
        PaginatedRequestParams, ReadResourceDirectoryRequestParams, ReadResourceDirectoryResult,
        ReadResourceRequestParams, ReadResourceResponse, ServerCapabilities, ServerConfig,
    },
    service::RequestContext,
    transport::stdio,
};
use tracing_subscriber::EnvFilter;

#[derive(Clone)]
struct SkillServer {
    skills: Arc<SkillCatalog>,
}

impl ServerHandler for SkillServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_resources()
                .enable_skills_with_directory_read()
                .build(),
        )
    }

    async fn list_skills(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListSkillsResult, ErrorData> {
        Ok(self.skills.list_skills())
    }

    async fn get_skill(
        &self,
        request: GetSkillRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetSkillResult, ErrorData> {
        self.skills.get_skill(&request.uri)
    }

    async fn read_resource_directory(
        &self,
        request: ReadResourceDirectoryRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceDirectoryResult, ErrorData> {
        self.skills.read_resource_directory(&request.uri)
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult::with_all_items(self.skills.resources()))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        self.skills
            .read_resource(&request.uri)
            .map(Into::into)
            .ok_or_else(|| ErrorData::resource_not_found(request.uri, None))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("skills"));
    let skills = SkillCatalog::from_dir(&root)?;
    tracing::info!(
        root = %root.display(),
        skills = skills.skills().len(),
        "Starting skills MCP server"
    );

    let service = SkillServer {
        skills: Arc::new(skills),
    }
    .serve(stdio())
    .await
    .inspect_err(|e| tracing::error!("serving error: {e:?}"))?;

    service.waiting().await?;
    Ok(())
}
