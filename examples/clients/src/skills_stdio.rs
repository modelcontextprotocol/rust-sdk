//! Client for the skills server in `examples/servers/src/skills_stdio.rs`.
//!
//! Walks through the SEP-2640 Skills extension from the host side, fetching
//! files only once they are needed:
//!   1. Discover the server on the `2026-07-28` revision the extension requires.
//!   2. List the skills (`skills/list`) and select `pdf-processing`.
//!   3. Fetch its manifest (`skills/get`), then load `SKILL.md` and verify its
//!      size, digest, and frontmatter against the manifest.
//!   4. Browse `templates/` with `resources/directory/read`, which only runs
//!      when the server declared `directoryRead`.
//!   5. Read the selected template, verified against the held manifest.

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine, prelude::BASE64_STANDARD};
use rmcp::{
    ClientLifecycleMode, ClientServiceExt, Peer, RoleClient,
    model::{
        ClientCapabilities, ClientConfig, GetSkillRequestParams, Implementation, ProtocolVersion,
        ReadResourceDirectoryRequestParams, ReadResourceRequestParams, ResourceContents,
        SkillFrontmatter, SkillResource, SkillResources,
    },
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use tokio::process::Command;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const SKILL_NAME: &str = "pdf-processing";
const TEMPLATES: &str = "skill://pdf-processing/templates";
const INVOICE: &str = "skill://pdf-processing/templates/invoice.md";

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
        .serve_with_lifecycle(
            TokioChildProcess::new(Command::new("cargo").configure(|cmd| {
                cmd.arg("run")
                    .arg("-q")
                    .arg("-p")
                    .arg("mcp-server-examples")
                    .arg("--example")
                    .arg("servers_skills_stdio");
            }))?,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await?;

    let skills = client.list_all_skills().await?;
    tracing::info!(count = skills.len(), "skills/list");
    let skill = skills
        .iter()
        .find(|skill| skill.name() == SKILL_NAME)
        .ok_or_else(|| anyhow!("the server has no {SKILL_NAME} skill"))?;

    let manifest = client
        .get_skill(GetSkillRequestParams::new(skill.uri.clone()))
        .await?
        .skill;
    let SkillResources::Files(files) = &manifest.resources else {
        bail!("{SKILL_NAME} has dynamic resources, so nothing can be verified");
    };

    let skill_md = read_verified(&client, files, &manifest.uri).await?;
    let frontmatter = parse_frontmatter(&skill_md)?;
    if frontmatter != manifest.frontmatter {
        bail!("{} frontmatter does not match skills/get", manifest.uri);
    }
    tracing::info!(name = manifest.name(), "loaded SKILL.md");

    let server_info = client
        .peer_info()
        .ok_or_else(|| anyhow!("server sent no discover result"))?;
    if server_info.capabilities.supports_directory_read() {
        let listing = client
            .read_resource_directory(ReadResourceDirectoryRequestParams::new(TEMPLATES))
            .await?;
        for entry in &listing.resources {
            let kind = if entry.is_directory() { "dir" } else { "file" };
            tracing::info!(uri = %entry.uri, kind, "resources/directory/read");
        }
    }

    let invoice = read_verified(&client, files, INVOICE).await?;
    tracing::info!(uri = INVOICE, size = invoice.len(), "loaded template");

    client.cancel().await?;
    Ok(())
}

/// Read `uri` and check its bytes against the manifest entry for it.
async fn read_verified(
    client: &Peer<RoleClient>,
    files: &[SkillResource],
    uri: &str,
) -> Result<Vec<u8>> {
    let file = files
        .iter()
        .find(|file| file.uri == uri)
        .ok_or_else(|| anyhow!("{uri} is not in the manifest"))?;
    let result = client
        .read_resource(ReadResourceRequestParams::new(uri))
        .await?;
    let bytes = match result.contents.first() {
        Some(ResourceContents::TextResourceContents { text, .. }) => text.clone().into_bytes(),
        Some(ResourceContents::BlobResourceContents { blob, .. }) => {
            BASE64_STANDARD.decode(blob)?
        }
        _ => bail!("{uri} came back without contents"),
    };
    file.verify(&bytes)?;
    Ok(bytes)
}

/// Parse the YAML block between the leading `---` lines of a `SKILL.md`.
fn parse_frontmatter(bytes: &[u8]) -> Result<SkillFrontmatter> {
    let text = std::str::from_utf8(bytes).context("SKILL.md is not UTF-8")?;
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        bail!("SKILL.md does not start with a frontmatter block");
    }
    let mut yaml = String::new();
    for line in lines {
        if line.trim_end() == "---" {
            return Ok(yaml_serde::from_str(&yaml)?);
        }
        yaml.push_str(line);
        yaml.push('\n');
    }
    bail!("SKILL.md frontmatter has no closing ---")
}
