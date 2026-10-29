//! `SkillCatalog` over a real directory tree, and served through `Peer`.
#![cfg(not(feature = "local"))]
#![cfg(all(feature = "client", feature = "skills-fs"))]

use std::{fs, path::Path, sync::Arc};

use base64::{Engine, prelude::BASE64_STANDARD};
use rmcp::{
    ClientHandler, ErrorData, RoleClient, RoleServer, ServerHandler,
    handler::server::skill_catalog::{SKILL_FILE_LIMIT, SkillCatalog},
    model::{
        ClientConfig, DIRECTORY_MIME_TYPE, ErrorCode, GetSkillRequestParams, GetSkillResult,
        InitializeResult, ListResourcesResult, ListSkillsResult, PaginatedRequestParams,
        ProtocolVersion, ReadResourceDirectoryRequestParams, ReadResourceDirectoryResult,
        ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, ResourceContents,
        ServerCapabilities, ServerConfig, Skill, SkillResources,
    },
    service::{RequestContext, RunningService, serve_directly},
};
use tempfile::TempDir;
use tokio::task::JoinHandle;

const PDF_SKILL_MD: &str = "---\nname: pdf-processing\ndescription: Extract, fill, and assemble PDF documents\nmetadata:\n  version: \"1.2\"\n---\n\n# PDF processing\n";
const BILLING_SKILL_MD: &str = "---\nname: billing\ndescription: Billing workflows\n---\n";
const REFUNDS_SKILL_MD: &str = "---\nname: refunds\ndescription: Refund workflows\n---\n";

const PDF_SKILL_URI: &str = "skill://pdf-processing/SKILL.md";
const INVOICE_URI: &str = "skill://pdf-processing/templates/invoice.md";
const TEMPLATES_DIR_URI: &str = "skill://pdf-processing/templates";
const BILLING_SKILL_URI: &str = "skill://acme/billing/SKILL.md";
const REFUNDS_SKILL_URI: &str = "skill://acme/billing/refunds/SKILL.md";
const REFUND_EMAIL_URI: &str = "skill://acme/billing/refunds/examples/email.md";

fn write(root: &Path, path: &str, contents: impl AsRef<[u8]>) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");
    fs::write(path, contents).expect("write file");
}

/// `pdf-processing` with a template folder, and `acme/billing` holding a
/// nested `refunds` skill.
fn skills_tree() -> TempDir {
    let dir = TempDir::new().expect("temp dir");
    let root = dir.path();
    write(root, "pdf-processing/SKILL.md", PDF_SKILL_MD);
    write(root, "pdf-processing/templates/invoice.md", "# Invoice\n");
    write(root, "acme/billing/SKILL.md", BILLING_SKILL_MD);
    write(root, "acme/billing/refunds/SKILL.md", REFUNDS_SKILL_MD);
    write(
        root,
        "acme/billing/refunds/examples/email.md",
        "Dear customer\n",
    );
    dir
}

fn load(dir: &TempDir) -> SkillCatalog {
    SkillCatalog::from_dir(dir.path()).expect("catalog loads")
}

fn uris(skills: &[Skill]) -> Vec<&str> {
    skills.iter().map(|skill| skill.uri.as_str()).collect()
}

fn manifest_uris(skill: &Skill) -> Vec<&str> {
    match &skill.resources {
        SkillResources::Files(files) => files.iter().map(|file| file.uri.as_str()).collect(),
        other => panic!("expected a file manifest, got {other:?}"),
    }
}

fn skill<'a>(skills: &'a [Skill], uri: &str) -> &'a Skill {
    skills
        .iter()
        .find(|skill| skill.uri == uri)
        .unwrap_or_else(|| panic!("no skill {uri}"))
}

fn read_bytes(catalog: &SkillCatalog, uri: &str) -> Vec<u8> {
    let result = catalog.read_resource(uri).expect("served");
    match &result.contents[0] {
        ResourceContents::TextResourceContents { text, .. } => text.as_bytes().to_vec(),
        ResourceContents::BlobResourceContents { blob, .. } => {
            BASE64_STANDARD.decode(blob).expect("base64")
        }
        other => panic!("unexpected contents {other:?}"),
    }
}

mod loading {
    use super::*;

    #[test]
    fn should_list_every_skill_ordered_by_uri_including_nested_ones() {
        let dir = skills_tree();

        let skills = load(&dir).skills();

        assert_eq!(
            uris(&skills),
            [BILLING_SKILL_URI, REFUNDS_SKILL_URI, PDF_SKILL_URI]
        );
    }

    #[test]
    fn should_list_nested_skill_files_in_the_parent_manifest() {
        let dir = skills_tree();

        let skills = load(&dir).skills();

        assert_eq!(
            manifest_uris(skill(&skills, BILLING_SKILL_URI)),
            [BILLING_SKILL_URI, REFUNDS_SKILL_URI, REFUND_EMAIL_URI]
        );
    }

    #[test]
    fn should_keep_extra_frontmatter_fields() {
        let dir = skills_tree();

        let skills = load(&dir).skills();

        assert_eq!(
            skill(&skills, PDF_SKILL_URI).frontmatter.extra["metadata"],
            serde_json::json!({ "version": "1.2" })
        );
    }

    #[test]
    fn should_ignore_hidden_entries() {
        let dir = skills_tree();
        write(dir.path(), "pdf-processing/.DS_Store", "junk");

        let skills = load(&dir).skills();

        assert_eq!(
            manifest_uris(skill(&skills, PDF_SKILL_URI)),
            [PDF_SKILL_URI, INVOICE_URI]
        );
    }

    #[test]
    fn should_skip_a_skill_whose_name_does_not_match_its_folder() {
        let dir = skills_tree();
        write(
            dir.path(),
            "renamed/SKILL.md",
            "---\nname: original\ndescription: Moved folder\n---\n",
        );

        let skills = load(&dir).skills();

        assert_eq!(
            uris(&skills),
            [BILLING_SKILL_URI, REFUNDS_SKILL_URI, PDF_SKILL_URI]
        );
    }

    #[test]
    fn should_skip_a_skill_without_frontmatter() {
        let dir = skills_tree();
        write(dir.path(), "plain/SKILL.md", "# No frontmatter\n");

        let skills = load(&dir).skills();

        assert!(!uris(&skills).contains(&"skill://plain/SKILL.md"));
    }

    #[test]
    fn should_skip_a_skill_over_the_file_limit() {
        let dir = skills_tree();
        for index in 0..SKILL_FILE_LIMIT {
            write(
                dir.path(),
                &format!("pdf-processing/pages/{index}.md"),
                "page\n",
            );
        }

        let skills = load(&dir).skills();

        assert!(!uris(&skills).contains(&PDF_SKILL_URI));
    }

    #[test]
    fn should_skip_a_skill_at_the_root() {
        let dir = TempDir::new().expect("temp dir");
        write(
            dir.path(),
            "SKILL.md",
            "---\nname: root\ndescription: At the root\n---\n",
        );

        let skills = load(&dir).skills();

        assert!(skills.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn should_skip_a_skill_that_contains_a_symlink() {
        let dir = skills_tree();
        let outside = TempDir::new().expect("temp dir");
        write(outside.path(), "secret.txt", "do not serve");
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            dir.path().join("pdf-processing/templates/secret.txt"),
        )
        .expect("symlink");

        let catalog = load(&dir);

        assert!(!uris(&catalog.skills()).contains(&PDF_SKILL_URI));
    }

    #[cfg(unix)]
    #[test]
    fn should_not_serve_files_through_a_symlinked_folder() {
        let dir = skills_tree();
        let outside = TempDir::new().expect("temp dir");
        write(outside.path(), "secret.txt", "do not serve");
        std::os::unix::fs::symlink(outside.path(), dir.path().join("pdf-processing/linked"))
            .expect("symlink");

        let catalog = load(&dir);

        assert!(
            catalog
                .read_resource("skill://pdf-processing/linked/secret.txt")
                .is_none()
        );
    }

    #[test]
    fn should_percent_encode_names_outside_the_uri_safe_set() {
        let dir = skills_tree();
        write(dir.path(), "pdf-processing/my notes.md", "notes\n");

        let skills = load(&dir).skills();

        assert!(
            manifest_uris(skill(&skills, PDF_SKILL_URI))
                .contains(&"skill://pdf-processing/my%20notes.md")
        );
    }

    #[test]
    fn should_fail_when_the_root_is_missing() {
        let dir = TempDir::new().expect("temp dir");

        let result = SkillCatalog::from_dir(dir.path().join("missing"));

        assert!(result.is_err());
    }
}

mod serving {
    use super::*;

    #[test]
    fn should_serve_bytes_that_match_every_manifest_entry() {
        let dir = skills_tree();
        let catalog = load(&dir);

        for skill in catalog.skills() {
            let SkillResources::Files(files) = &skill.resources else {
                panic!("expected a file manifest");
            };
            for file in files {
                file.verify(&read_bytes(&catalog, &file.uri))
                    .unwrap_or_else(|error| panic!("{}: {error}", file.uri));
            }
        }
    }

    #[test]
    fn should_serve_the_loaded_bytes_after_the_file_changes_on_disk() {
        let dir = skills_tree();
        let catalog = load(&dir);
        write(
            dir.path(),
            "pdf-processing/templates/invoice.md",
            "changed\n",
        );

        let bytes = read_bytes(&catalog, INVOICE_URI);

        assert_eq!(bytes, b"# Invoice\n");
    }

    #[test]
    fn should_serve_a_binary_type_as_a_blob_even_when_it_is_valid_utf8() {
        let dir = skills_tree();
        write(dir.path(), "pdf-processing/logo.png", "PNG");
        let catalog = load(&dir);

        let result = catalog
            .read_resource("skill://pdf-processing/logo.png")
            .expect("served");

        assert!(matches!(
            &result.contents[0],
            ResourceContents::BlobResourceContents { .. }
        ));
    }

    #[test]
    fn should_serve_non_utf8_files_as_blobs() {
        let dir = skills_tree();
        write(dir.path(), "pdf-processing/logo.png", [0x89, b'P', 0xff]);
        let catalog = load(&dir);

        let result = catalog
            .read_resource("skill://pdf-processing/logo.png")
            .expect("served");

        assert!(matches!(
            &result.contents[0],
            ResourceContents::BlobResourceContents { mime_type: Some(mime), .. } if mime == "image/png"
        ));
    }

    #[test]
    fn should_return_none_for_a_uri_it_does_not_serve() {
        let dir = skills_tree();

        let result = load(&dir).read_resource("skill://pdf-processing/missing.md");

        assert!(result.is_none());
    }

    #[test]
    fn should_reject_an_unknown_skill_with_invalid_params() {
        let dir = skills_tree();

        let error = load(&dir)
            .get_skill("skill://unknown/SKILL.md")
            .expect_err("unknown skill");

        assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
    }

    #[test]
    fn should_list_directory_children_with_folders_marked() {
        let dir = skills_tree();
        write(
            dir.path(),
            "pdf-processing/templates/regional/eu.md",
            "# EU\n",
        );

        let children = load(&dir)
            .read_resource_directory(TEMPLATES_DIR_URI)
            .expect("directory")
            .resources;

        let listed: Vec<(&str, Option<&str>)> = children
            .iter()
            .map(|child| (child.uri.as_str(), child.mime_type.as_deref()))
            .collect();
        assert_eq!(
            listed,
            [
                (INVOICE_URI, Some("text/markdown")),
                (
                    "skill://pdf-processing/templates/regional",
                    Some(DIRECTORY_MIME_TYPE)
                ),
            ]
        );
    }

    #[test]
    fn should_reject_a_directory_read_of_a_file() {
        let dir = skills_tree();

        let error = load(&dir)
            .read_resource_directory(INVOICE_URI)
            .expect_err("a file");

        assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
    }

    #[test]
    fn should_list_one_skill_md_resource_per_skill() {
        let dir = skills_tree();

        let resources = load(&dir).resources();

        let listed: Vec<&str> = resources
            .iter()
            .map(|resource| resource.uri.as_str())
            .collect();
        assert_eq!(
            listed,
            [BILLING_SKILL_URI, REFUNDS_SKILL_URI, PDF_SKILL_URI]
        );
    }
}

mod reloading {
    use super::*;

    #[test]
    fn should_pick_up_a_new_skill() {
        let dir = skills_tree();
        let catalog = load(&dir);
        write(
            dir.path(),
            "notes/SKILL.md",
            "---\nname: notes\ndescription: Notes\n---\n",
        );

        catalog.reload().expect("reload");

        assert!(uris(&catalog.skills()).contains(&"skill://notes/SKILL.md"));
    }

    #[test]
    fn should_report_a_change() {
        let dir = skills_tree();
        let catalog = load(&dir);
        write(
            dir.path(),
            "pdf-processing/templates/invoice.md",
            "changed\n",
        );

        let changed = catalog.reload().expect("reload");

        assert!(changed);
    }

    #[test]
    fn should_report_no_change_for_an_identical_tree() {
        let dir = skills_tree();
        let catalog = load(&dir);

        let changed = catalog.reload().expect("reload");

        assert!(!changed);
    }

    #[test]
    fn should_keep_serving_the_old_snapshot_when_reload_fails() {
        let dir = skills_tree();
        let catalog = load(&dir);
        fs::remove_dir_all(dir.path()).expect("remove tree");
        catalog.reload().expect_err("root is gone");

        let skills = catalog.skills();

        assert_eq!(
            uris(&skills),
            [BILLING_SKILL_URI, REFUNDS_SKILL_URI, PDF_SKILL_URI]
        );
    }
}

/// Serves a catalog the way an application would wire it up.
#[derive(Debug, Clone)]
struct CatalogServer {
    catalog: Arc<SkillCatalog>,
}

impl ServerHandler for CatalogServer {
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
        Ok(self.catalog.list_skills())
    }

    async fn get_skill(
        &self,
        request: GetSkillRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetSkillResult, ErrorData> {
        self.catalog.get_skill(&request.uri)
    }

    async fn read_resource_directory(
        &self,
        request: ReadResourceDirectoryRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceDirectoryResult, ErrorData> {
        self.catalog.read_resource_directory(&request.uri)
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult::with_all_items(
            self.catalog.resources(),
        ))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        self.catalog
            .read_resource(&request.uri)
            .map(Into::into)
            .ok_or_else(|| ErrorData::resource_not_found(request.uri, None))
    }
}

#[derive(Debug, Clone, Default)]
struct DefaultClient;

impl ClientHandler for DefaultClient {
    fn get_info(&self) -> ClientConfig {
        let mut info = ClientConfig::default();
        info.protocol_version = ProtocolVersion::V_2026_07_28;
        info
    }
}

type Client = RunningService<RoleClient, DefaultClient>;
type ServerTask = JoinHandle<anyhow::Result<()>>;

/// Serves the catalog of `dir` to a client on 2026-07-28 over a duplex pipe.
fn connect(dir: &TempDir) -> (Client, ServerTask) {
    let server = CatalogServer {
        catalog: Arc::new(load(dir)),
    };
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let mut server_info = InitializeResult::new(server.get_info().capabilities);
    server_info.protocol_version = ProtocolVersion::V_2026_07_28;
    let server = serve_directly::<RoleServer, _, _, _, _>(
        server,
        server_transport,
        Some(DefaultClient.get_info()),
    );
    let server_task = tokio::spawn(async move {
        server.waiting().await?;
        anyhow::Ok(())
    });
    let client = serve_directly::<RoleClient, _, _, _, _>(
        DefaultClient,
        client_transport,
        Some(server_info.into()),
    );
    (client, server_task)
}

async fn disconnect(client: Client, server_task: ServerTask) {
    client.cancel().await.expect("cancel");
    server_task.await.expect("join").expect("server");
}

#[tokio::test]
async fn a_client_should_list_every_catalog_skill() {
    let dir = skills_tree();
    let (client, server_task) = connect(&dir);

    let skills = client.list_all_skills().await.expect("list_all_skills");
    disconnect(client, server_task).await;

    assert_eq!(
        uris(&skills),
        [BILLING_SKILL_URI, REFUNDS_SKILL_URI, PDF_SKILL_URI]
    );
}

#[tokio::test]
async fn a_client_should_read_a_catalog_directory() {
    let dir = skills_tree();
    let (client, server_task) = connect(&dir);

    let children = client
        .read_resource_directory(ReadResourceDirectoryRequestParams::new(TEMPLATES_DIR_URI))
        .await
        .expect("read_resource_directory")
        .resources;
    disconnect(client, server_task).await;

    let listed: Vec<&str> = children.iter().map(|child| child.uri.as_str()).collect();
    assert_eq!(listed, [INVOICE_URI]);
}

#[tokio::test]
async fn a_client_should_verify_every_file_of_a_catalog_skill() {
    let dir = skills_tree();
    let (client, server_task) = connect(&dir);

    let skill = client
        .get_skill(GetSkillRequestParams::new(REFUNDS_SKILL_URI))
        .await
        .expect("get_skill")
        .skill;
    let SkillResources::Files(files) = skill.resources else {
        panic!("expected a file manifest");
    };
    let mut verified = Vec::new();
    for file in &files {
        let ReadResourceResult { contents, .. } = client
            .read_resource(ReadResourceRequestParams::new(&file.uri))
            .await
            .expect("read_resource");
        let ResourceContents::TextResourceContents { text, .. } = &contents[0] else {
            panic!("expected text");
        };
        verified.push(file.verify(text.as_bytes()).is_ok());
    }
    disconnect(client, server_task).await;

    assert_eq!(verified, [true, true]);
}
