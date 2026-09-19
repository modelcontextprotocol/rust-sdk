//! File-system-backed MCP skill server over stdio.
//!
//! Usage:
//!   cargo run --example skill_fs_server --features "server,transport-io" -- --root ./fixtures/skills-dir
//!
//! Then with MCP Inspector:
//!   npx @modelcontextprotocol/inspector --cli \
//!     --transport stdio \
//!     --command "cargo run --example skill_fs_server --features server,transport-io -- --root ./fixtures/skills-dir" \
//!     --method skills/list

use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
};

use rmcp::{
    ServerHandler, ServiceExt,
    model::skills::{
        self, DirectoryEntry, SkillEntry, SkillResource, SkillResources, SkillsGetRequestParams,
        SkillsGetResult, SkillsListResult,
    },
    service::{RequestContext, RoleServer},
};

const SKILL_FILE: &str = "SKILL.md";
const DEFAULT_CACHE_TTL_MS: u64 = 600_000;

/// A file-system-backed MCP server that auto-discovers skills from a directory tree.
pub struct FileSystemSkillServer {
    root: PathBuf,
    skills: HashMap<String, SkillEntry>,
}

impl FileSystemSkillServer {
    /// Walk `root` for `**/SKILL.md` files and build the skill catalog.
    pub fn new(root: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        let root = root.as_ref().to_path_buf();
        let mut skills = HashMap::new();
        Self::walk_dir(&root, &root, &mut skills)?;
        Ok(Self { root, skills })
    }

    fn walk_dir(
        root: &Path,
        dir: &Path,
        skills: &mut HashMap<String, SkillEntry>,
    ) -> Result<(), std::io::Error> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                Self::walk_dir(root, &path, skills)?;
            } else if path.file_name().and_then(|s| s.to_str()) == Some(SKILL_FILE)
                && let Some(skill) = Self::parse_skill_file(root, &path)
            {
                skills.insert(skill.uri.clone(), skill);
            }
        }
        Ok(())
    }

    fn parse_skill_file(root: &Path, path: &Path) -> Option<SkillEntry> {
        let content = fs::read_to_string(path).ok()?;
        let (frontmatter, _) = Self::split_frontmatter(&content)?;
        let frontmatter: serde_json::Value = Self::parse_simple_yaml(&frontmatter)?;

        // Canonicalize and verify the path stays within root
        let canonical_root = root.canonicalize().ok()?;
        let canonical_path = path.canonicalize().ok()?;
        if !canonical_path.starts_with(&canonical_root) {
            return None;
        }

        let relative = canonical_path.strip_prefix(&canonical_root).ok()?;
        let uri = format!("skill://{}", relative.to_string_lossy());

        let parent_dir = canonical_path.parent()?;
        let mut resources = Vec::new();
        if let Ok(entries) = fs::read_dir(parent_dir) {
            for entry in entries.flatten() {
                let sibling = entry.path();
                if sibling == canonical_path {
                    continue;
                }
                let name = sibling.file_name().and_then(|s| s.to_str())?;
                if name == SKILL_FILE || sibling.is_dir() {
                    continue;
                }
                // Canonicalize sibling and verify it stays within root
                let canonical_sibling = sibling.canonicalize().ok()?;
                if !canonical_sibling.starts_with(&canonical_root) {
                    continue;
                }
                let sibling_relative = canonical_sibling.strip_prefix(&canonical_root).ok()?;
                let sibling_uri = format!("skill://{}", sibling_relative.to_string_lossy());
                let size = fs::metadata(&canonical_sibling)
                    .map(|m| m.len())
                    .unwrap_or(0);
                let digest = Self::compute_sha256(&canonical_sibling);
                resources.push(SkillResource::new(sibling_uri, digest, size));
            }
        }

        let resources = if resources.is_empty() {
            None
        } else {
            Some(SkillResources::FileList(resources))
        };

        // meta is None because this is a static file-based server.
        // The spec's _meta field (ttlMs, cacheScope) is optional on skills/get
        // and only required on skills/list for protocol 2026-07-28+.
        // FileSystemSkillServer does not implement caching, so meta is left None.
        let mut entry = SkillEntry::new(uri, frontmatter);
        entry.resources = resources;

        Some(entry)
    }

    /// Compute SHA-256 digest of a file, returning "sha256:<hex>" format.
    fn compute_sha256(path: &Path) -> String {
        use std::io::Read;

        use sha2::{Digest, Sha256};
        let mut file = match fs::File::open(path) {
            Ok(f) => f,
            Err(_) => return String::new(),
        };
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 8192];
        loop {
            match file.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => hasher.update(&buffer[..n]),
                Err(_) => return String::new(),
            }
        }
        let result = hasher.finalize();
        // Format as "sha256:<hex>" using fmt to avoid needing hex crate
        let mut hex_string = String::with_capacity(64);
        for byte in result {
            use std::fmt::Write;
            let _ = write!(hex_string, "{:02x}", byte);
        }
        format!("sha256:{}", hex_string)
    }

    fn split_frontmatter(content: &str) -> Option<(String, String)> {
        let content = content.strip_prefix("---\n")?;
        let mut lines = content.lines();
        let mut fm = Vec::new();
        for line in &mut lines {
            if line.trim() == "---" {
                break;
            }
            fm.push(line);
        }
        let body: Vec<&str> = lines.collect();
        Some((fm.join("\n"), body.join("\n")))
    }

    /// Parse simple flat YAML frontmatter into a JSON object.
    ///
    /// **Limitation**: Only flat string fields are supported. Values containing
    /// colons are handled correctly (split on first `:` only). Quoted values
    /// have their outer quotes stripped. Lines without a colon are rejected
    /// (returns `None`). Empty lines and comments (`#`) are skipped.
    fn parse_simple_yaml(yaml: &str) -> Option<serde_json::Value> {
        let mut map = serde_json::Map::new();
        for line in yaml.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line.split_once(':')?;
            let key = key.trim().to_string();
            if key.is_empty() {
                return None;
            }
            let value = value.trim();
            // Strip one layer of matching quotes (double or single)
            let value = if (value.starts_with('"') && value.ends_with('"') && value.len() >= 2)
                || (value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2)
            {
                &value[1..value.len() - 1]
            } else {
                value
            };
            map.insert(key, serde_json::Value::String(value.to_string()));
        }
        Some(serde_json::Value::Object(map))
    }

    /// List all discovered skills, sorted by URI.
    pub fn list_skills(&self) -> Vec<&SkillEntry> {
        let mut skills: Vec<_> = self.skills.values().collect();
        skills.sort_by(|a, b| a.uri.cmp(&b.uri));
        skills
    }

    /// Get a single skill by its URI.
    pub fn get_skill(&self, uri: &str) -> Option<&SkillEntry> {
        self.skills.get(uri)
    }

    /// List the sibling files of a skill file URI.
    pub fn list_skill_directory(&self, uri: &str) -> Option<Vec<DirectoryEntry>> {
        let path = uri.strip_prefix("skill://")?;
        let file_path = self.root.join(path);
        let parent = file_path.parent()?;

        // Canonicalize and verify the path stays within root
        let canonical_root = self.root.canonicalize().ok()?;
        let canonical_parent = parent.canonicalize().ok()?;
        if !canonical_parent.starts_with(&canonical_root) {
            return None;
        }

        let mut entries = Vec::new();
        for entry in fs::read_dir(&canonical_parent).ok()?.flatten() {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            if name == SKILL_FILE {
                continue;
            }
            // Canonicalize each entry and verify it stays within root
            let canonical_entry = path.canonicalize().ok()?;
            if !canonical_entry.starts_with(&canonical_root) {
                continue;
            }
            let relative = canonical_entry.strip_prefix(&canonical_root).ok()?;
            let entry_uri = format!("skill://{}", relative.to_string_lossy());
            entries.push(if canonical_entry.is_dir() {
                DirectoryEntry::dir(entry_uri, name.to_string())
            } else {
                DirectoryEntry::file(entry_uri, name.to_string())
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Some(entries)
    }
}

impl ServerHandler for FileSystemSkillServer {
    async fn skills_list(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<SkillsListResult, rmcp::ErrorData> {
        let mut skills: Vec<SkillEntry> = self.skills.values().cloned().collect();
        skills.sort_by(|a, b| a.uri.cmp(&b.uri));
        Ok(SkillsListResult::new(skills))
    }

    async fn skills_get(
        &self,
        request: SkillsGetRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<SkillsGetResult, rmcp::ErrorData> {
        match self.skills.get(&request.uri) {
            Some(skill) => Ok(SkillsGetResult::new(skill.clone())),
            None => Err(rmcp::ErrorData::invalid_params(
                format!("skill not found: {}", request.uri),
                None,
            )),
        }
    }

    async fn resources_directory_read(
        &self,
        request: skills::ResourcesDirectoryReadRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<skills::ResourcesDirectoryReadResult, rmcp::ErrorData> {
        let children = self.list_skill_directory(&request.uri).unwrap_or_default();
        let supports_cache_hints = context
            .protocol_version()
            .as_ref()
            .is_some_and(|v| v.as_str() >= rmcp::model::ProtocolVersion::V_2026_07_28.as_str());
        let mut result = skills::ResourcesDirectoryReadResult::new(children);
        if supports_cache_hints {
            result.ttl_ms = Some(DEFAULT_CACHE_TTL_MS);
            result.cache_scope = Some("public".to_string());
        }
        Ok(result)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let mut root = None;
    while let Some(arg) = args.next() {
        if arg == "--root" {
            root = args.next();
        }
    }
    let root: PathBuf = root
        .unwrap_or_else(|| "./fixtures/skills-dir".to_string())
        .into();
    let server = FileSystemSkillServer::new(&root)
        .map_err(|e| format!("failed to load skills from {}: {}", root.display(), e))?;
    let transport = rmcp::transport::stdio();
    let service = server.serve(transport).await?;
    service.waiting().await?;
    Ok(())
}
