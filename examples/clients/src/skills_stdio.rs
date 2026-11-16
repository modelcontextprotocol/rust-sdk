//! Client for the skills server in `examples/servers/src/skills_stdio.rs`.
//!
//! Walks through the SEP-2640 Skills extension from the host side:
//!   1. List every skill the server declares (`skills/list`).
//!   2. Fetch each manifest (`skills/get`) and download every file it lists
//!      over `resources/read`, verifying the bytes against the manifest's
//!      size and SHA-256 digest.
//!   3. Browse a skill folder with `resources/directory/read`, which only runs
//!      when the server declared `directoryRead`.

use anyhow::{Result, anyhow};
use base64::{Engine, prelude::BASE64_STANDARD};
use rmcp::{
    ServiceExt,
    model::{
        ClientCapabilities, ClientConfig, GetSkillRequestParams, Implementation,
        ReadResourceDirectoryRequestParams, ReadResourceRequestParams, ResourceContents,
        SkillResources,
    },
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use tokio::process::Command;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| format!("info,{}=debug", env!("CARGO_CRATE_NAME")).into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let client_info = ClientConfig::new(
        ClientCapabilities::default(),
        Implementation::from_build_env(),
    );
    let client = client_info
        .serve(TokioChildProcess::new(Command::new("cargo").configure(
            |cmd| {
                cmd.arg("run")
                    .arg("-q")
                    .arg("-p")
                    .arg("mcp-server-examples")
                    .arg("--example")
                    .arg("servers_skills_stdio");
            },
        ))?)
        .await?;

    let skills = client.list_all_skills().await?;
    tracing::info!(count = skills.len(), "skills/list");

    for skill in &skills {
        let manifest = client
            .get_skill(GetSkillRequestParams::new(skill.uri.clone()))
            .await?
            .skill;
        tracing::info!(
            name = manifest.name(),
            description = manifest.frontmatter.description,
            "skills/get"
        );

        let SkillResources::Files(files) = &manifest.resources else {
            tracing::info!(uri = %skill.uri, "resources are dynamic; browse the directory instead");
            continue;
        };
        for file in files {
            let result = client
                .read_resource(ReadResourceRequestParams::new(file.uri.clone()))
                .await?;
            let bytes = match result.contents.first() {
                Some(ResourceContents::TextResourceContents { text, .. }) => {
                    text.clone().into_bytes()
                }
                Some(ResourceContents::BlobResourceContents { blob, .. }) => {
                    BASE64_STANDARD.decode(blob)?
                }
                _ => return Err(anyhow!("{} came back without contents", file.uri)),
            };
            file.verify(&bytes)?;
            tracing::info!(uri = %file.uri, size = file.size, "verified");
        }
    }

    let server_info = client
        .peer_info()
        .ok_or_else(|| anyhow!("server sent no initialize result"))?;
    if server_info.capabilities.supports_directory_read() {
        let folder = "skill://pdf-processing/templates";
        let listing = client
            .read_resource_directory(ReadResourceDirectoryRequestParams::new(folder))
            .await?;
        for entry in &listing.resources {
            let kind = if entry.is_directory() { "dir" } else { "file" };
            tracing::info!(uri = %entry.uri, kind, "resources/directory/read");
        }
    }

    client.cancel().await?;
    Ok(())
}
