//! Skills served from a directory tree (SEP-2640).
//!
//! Each folder below the root that holds a `SKILL.md` is a skill, served at
//! `skill://<path from the root>/SKILL.md`. A folder of skills can nest other
//! skills: the nested skill is listed on its own, and its files also appear
//! in the manifest of every skill above it.
//!
//! A server delegates the skills methods and `resources/read` to the catalog:
//!
//! ```rust,no_run
//! use std::sync::Arc;
//!
//! use rmcp::{
//!     ErrorData, RoleServer, ServerHandler,
//!     handler::server::skill_catalog::SkillCatalog,
//!     model::*,
//!     service::RequestContext,
//! };
//!
//! #[derive(Clone)]
//! struct Server {
//!     skills: Arc<SkillCatalog>,
//! }
//!
//! impl ServerHandler for Server {
//!     fn get_info(&self) -> ServerConfig {
//!         ServerConfig::new(
//!             ServerCapabilities::builder()
//!                 .enable_resources()
//!                 .enable_skills_with_directory_read()
//!                 .build(),
//!         )
//!     }
//!
//!     async fn list_skills(
//!         &self,
//!         _request: Option<PaginatedRequestParams>,
//!         _context: RequestContext<RoleServer>,
//!     ) -> Result<ListSkillsResult, ErrorData> {
//!         Ok(self.skills.list_skills())
//!     }
//!
//!     async fn get_skill(
//!         &self,
//!         request: GetSkillRequestParams,
//!         _context: RequestContext<RoleServer>,
//!     ) -> Result<GetSkillResult, ErrorData> {
//!         self.skills.get_skill(&request.uri)
//!     }
//!
//!     async fn read_resource_directory(
//!         &self,
//!         request: ReadResourceDirectoryRequestParams,
//!         _context: RequestContext<RoleServer>,
//!     ) -> Result<ReadResourceDirectoryResult, ErrorData> {
//!         self.skills.read_resource_directory(&request.uri)
//!     }
//!
//!     async fn list_resources(
//!         &self,
//!         _request: Option<PaginatedRequestParams>,
//!         _context: RequestContext<RoleServer>,
//!     ) -> Result<ListResourcesResult, ErrorData> {
//!         Ok(ListResourcesResult::with_all_items(self.skills.resources()))
//!     }
//!
//!     async fn read_resource(
//!         &self,
//!         request: ReadResourceRequestParams,
//!         _context: RequestContext<RoleServer>,
//!     ) -> Result<ReadResourceResponse, ErrorData> {
//!         self.skills
//!             .read_resource(&request.uri)
//!             .map(Into::into)
//!             .ok_or_else(|| ErrorData::resource_not_found(request.uri, None))
//!     }
//! }
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let server = Server {
//!     skills: Arc::new(SkillCatalog::from_dir("./skills")?),
//! };
//! # let _ = server;
//! # Ok(())
//! # }
//! ```

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError, RwLock},
};

use base64::{Engine, prelude::BASE64_STANDARD};
use cap_std::{ambient_authority, fs::Dir};

use crate::{
    error::ErrorData as McpError,
    model::{
        DIRECTORY_MIME_TYPE, GetSkillResult, ListSkillsResult, ReadResourceDirectoryResult,
        ReadResourceResult, Resource, ResourceContents, SKILL_MD, Skill, SkillFrontmatter,
        SkillResource, SkillResources,
    },
};

/// Files per skill above which the catalog skips the skill; hosts need not load larger ones.
pub const SKILL_FILE_LIMIT: usize = 512;

/// Bytes per skill above which the catalog skips the skill; hosts need not load larger ones.
pub const SKILL_BYTE_LIMIT: u64 = 16 * 1024 * 1024;

const URI_SCHEME: &str = "skill://";
const MARKDOWN_MIME_TYPE: &str = "text/markdown";

/// An in-memory snapshot of the skills below a directory.
///
/// The snapshot holds every file's bytes, so `resources/read` serves exactly
/// the bytes the manifest describes even if the files change on disk. Call
/// [`reload`](Self::reload) to pick up changes.
///
/// A skill that contains a symlink or another special file is skipped with a
/// warning. Every listing and read goes through a handle on the root that
/// cannot resolve outside it, so no file outside the root is served, even if
/// the tree changes while it loads.
/// Entries whose name starts with `.` are ignored, and a skill over
/// [`SKILL_FILE_LIMIT`] files or [`SKILL_BYTE_LIMIT`] bytes is skipped. Skipped
/// skills are reported through `tracing`.
///
/// Loading reads the whole tree with blocking I/O; from async code, call it
/// inside `tokio::task::spawn_blocking`.
#[derive(Debug)]
pub struct SkillCatalog {
    root: PathBuf,
    snapshot: RwLock<Arc<Snapshot>>,
    /// Held for a whole reload, so an older load never replaces a newer one.
    reloading: Mutex<()>,
}

/// A failure that stops a catalog from loading.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SkillCatalogError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl SkillCatalog {
    /// Load every skill below `root`.
    pub fn from_dir(root: impl Into<PathBuf>) -> Result<Self, SkillCatalogError> {
        let root = root.into();
        let snapshot = Snapshot::load(&root)?;
        Ok(Self {
            root,
            snapshot: RwLock::new(Arc::new(snapshot)),
            reloading: Mutex::new(()),
        })
    }

    /// Load the tree again and swap the new snapshot in.
    ///
    /// On failure the previous snapshot keeps serving. Returns `true` when any
    /// skill entry changed, so the caller can send `resources/list_changed`.
    pub fn reload(&self) -> Result<bool, SkillCatalogError> {
        let _reloading = self
            .reloading
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let snapshot = Arc::new(Snapshot::load(&self.root)?);
        let mut current = self
            .snapshot
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let changed = current.skills != snapshot.skills;
        *current = snapshot;
        Ok(changed)
    }

    /// Every skill, ordered by URI.
    pub fn skills(&self) -> Vec<Skill> {
        self.current().skills.clone()
    }

    /// The answer to `skills/list`: every skill on one page.
    pub fn list_skills(&self) -> ListSkillsResult {
        ListSkillsResult::with_all_items(self.skills())
    }

    /// The answer to `skills/get`; `-32602` when no skill has that URI.
    pub fn get_skill(&self, uri: &str) -> Result<GetSkillResult, McpError> {
        self.current()
            .skills
            .iter()
            .find(|skill| skill.uri == uri)
            .map(|skill| GetSkillResult::new(skill.clone()))
            .ok_or_else(|| McpError::invalid_params(format!("unknown skill {uri}"), None))
    }

    /// The `SKILL.md` of every skill, for `resources/list`.
    pub fn resources(&self) -> Vec<Resource> {
        self.current().listed.clone()
    }

    /// The answer to `resources/read` for a skill file, or `None` when the
    /// catalog does not serve `uri`, so the caller can try its own resources.
    pub fn read_resource(&self, uri: &str) -> Option<ReadResourceResult> {
        let snapshot = self.current();
        let file = snapshot.files.get(uri)?;
        let text = is_textual(file.mime_type)
            .then(|| std::str::from_utf8(&file.bytes).ok())
            .flatten();
        let contents = match text {
            Some(text) => ResourceContents::text(text, uri),
            None => ResourceContents::blob(BASE64_STANDARD.encode(&file.bytes), uri),
        };
        Some(ReadResourceResult::new(vec![
            contents.with_mime_type(file.mime_type),
        ]))
    }

    /// The answer to `resources/directory/read`; `-32602` when `uri` is not a
    /// directory inside a skill.
    pub fn read_resource_directory(
        &self,
        uri: &str,
    ) -> Result<ReadResourceDirectoryResult, McpError> {
        self.current()
            .directories
            .get(uri)
            .map(|children| ReadResourceDirectoryResult::with_all_items(children.clone()))
            .ok_or_else(|| McpError::invalid_params(format!("not a directory: {uri}"), None))
    }

    fn current(&self) -> Arc<Snapshot> {
        self.snapshot
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[derive(Debug, Default)]
struct Snapshot {
    skills: Vec<Skill>,
    /// The `SKILL.md` resource of each skill, for `resources/list`.
    listed: Vec<Resource>,
    files: HashMap<String, CatalogFile>,
    directories: HashMap<String, Vec<Resource>>,
}

#[derive(Debug)]
struct CatalogFile {
    bytes: Arc<[u8]>,
    mime_type: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    File,
    Directory,
    /// A symlink, another special file, or a name that is not UTF-8.
    Unsupported,
}

#[derive(Debug)]
struct Entry {
    /// Relative to the root, `/`-separated.
    path: String,
    kind: Kind,
    /// Bytes on disk when walked; `0` for anything but a file.
    size: u64,
}

impl Snapshot {
    fn load(root: &Path) -> Result<Self, SkillCatalogError> {
        let handle = Dir::open_ambient_dir(root, ambient_authority()).map_err(|source| {
            SkillCatalogError::Io {
                path: root.to_path_buf(),
                source,
            }
        })?;
        let tree = walk(root, &handle)?;
        let skill_dirs: BTreeSet<&str> = tree
            .iter()
            .filter(|entry| is_manifest(entry))
            .map(|entry| parent(&entry.path))
            .collect();

        let mut inside: BTreeMap<&str, Vec<&Entry>> = BTreeMap::new();
        for entry in &tree {
            let mut in_skill = false;
            let mut dir = parent(&entry.path);
            while !dir.is_empty() {
                if skill_dirs.contains(dir) {
                    in_skill = true;
                    inside.entry(dir).or_default().push(entry);
                }
                dir = parent(dir);
            }
            if !in_skill && entry.kind == Kind::Unsupported && !is_manifest(entry) {
                tracing::warn!(path = %entry.path, "skipping a symlink or special file");
            }
        }

        // Skip what the walk already rules out before reading any bytes.
        let mut kept: Vec<(&str, &[&Entry])> = Vec::new();
        for dir in skill_dirs {
            if dir.is_empty() {
                tracing::warn!("skipping the SKILL.md at the catalog root; a skill needs a folder");
                continue;
            }
            let entries = inside.get(dir).map(Vec::as_slice).unwrap_or_default();
            match check_walked(entries) {
                Ok(()) => kept.push((dir, entries)),
                Err(reason) => tracing::warn!(skill = %dir, "skipping skill: {reason}"),
            }
        }

        let mut bytes_of: HashMap<&str, Arc<[u8]>> = HashMap::new();
        for entry in kept.iter().flat_map(|(_, entries)| entries.iter()) {
            if entry.kind == Kind::File && !bytes_of.contains_key(entry.path.as_str()) {
                let bytes =
                    read_bounded(&handle, &entry.path).map_err(|source| SkillCatalogError::Io {
                        path: root.join(&entry.path),
                        source,
                    })?;
                bytes_of.insert(&entry.path, bytes.into());
            }
        }

        let mut snapshot = Snapshot::default();
        let mut directories: BTreeSet<&str> = BTreeSet::new();
        for (dir, entries) in kept {
            let (skill, listing) = match build_skill(dir, entries, &bytes_of) {
                Ok(built) => built,
                Err(reason) => {
                    tracing::warn!(skill = %dir, "skipping skill: {reason}");
                    continue;
                }
            };
            directories.insert(dir);
            for entry in entries {
                match entry.kind {
                    Kind::Directory => {
                        directories.insert(&entry.path);
                    }
                    Kind::File => {
                        snapshot
                            .files
                            .entry(uri_of(&entry.path))
                            .or_insert_with(|| CatalogFile {
                                bytes: Arc::clone(&bytes_of[entry.path.as_str()]),
                                mime_type: mime_type_of(&entry.path),
                            });
                    }
                    Kind::Unsupported => {}
                }
            }
            snapshot.skills.push(skill);
            snapshot.listed.push(listing);
        }
        snapshot.skills.sort_by(|a, b| a.uri.cmp(&b.uri));
        snapshot.listed.sort_by(|a, b| a.uri.cmp(&b.uri));

        let mut children: HashMap<&str, Vec<&Entry>> = HashMap::new();
        for entry in &tree {
            children.entry(parent(&entry.path)).or_default().push(entry);
        }
        for dir in directories {
            let listing = children
                .get(dir)
                .into_iter()
                .flatten()
                .filter_map(|entry| directory_child(entry, &snapshot.files))
                .collect();
            snapshot.directories.insert(uri_of(dir), listing);
        }
        Ok(snapshot)
    }
}

fn directory_child(entry: &Entry, files: &HashMap<String, CatalogFile>) -> Option<Resource> {
    let uri = uri_of(&entry.path);
    let name = file_name(&entry.path);
    match entry.kind {
        Kind::Directory => Some(Resource::new(uri, name).with_mime_type(DIRECTORY_MIME_TYPE)),
        Kind::File => {
            let file = files.get(&uri)?;
            Some(
                Resource::new(uri, name)
                    .with_mime_type(file.mime_type)
                    .with_size(file.bytes.len() as u64),
            )
        }
        Kind::Unsupported => None,
    }
}

/// Rules that the walk alone can check: no special files, and within the limits.
fn check_walked(entries: &[&Entry]) -> Result<(), String> {
    if let Some(entry) = entries.iter().find(|entry| entry.kind == Kind::Unsupported) {
        return Err(format!(
            "{} is a symlink, a special file, or not UTF-8; copy the file instead",
            entry.path
        ));
    }
    let files = entries.iter().filter(|entry| entry.kind == Kind::File);
    check_limits(files.clone().count(), files.map(|entry| entry.size).sum())
}

fn check_limits(files: usize, bytes: u64) -> Result<(), String> {
    if files > SKILL_FILE_LIMIT || bytes > SKILL_BYTE_LIMIT {
        return Err(format!(
            "{files} files and {bytes} bytes exceed the limit of {SKILL_FILE_LIMIT} files or \
             {SKILL_BYTE_LIMIT} bytes, so hosts need not load it"
        ));
    }
    Ok(())
}

/// Read at most one byte past the skill limit, so a file that grew after the
/// walk cannot exhaust memory; the limit check on the bytes then rejects it.
fn read_bounded(handle: &Dir, path: &str) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    handle
        .open(path)?
        .take(SKILL_BYTE_LIMIT + 1)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn build_skill(
    dir: &str,
    entries: &[&Entry],
    bytes_of: &HashMap<&str, Arc<[u8]>>,
) -> Result<(Skill, Resource), String> {
    let manifest_path = format!("{dir}/{SKILL_MD}");
    let manifest = bytes_of
        .get(manifest_path.as_str())
        .ok_or_else(|| format!("{manifest_path} is not a regular file"))?;
    let frontmatter = parse_frontmatter(manifest)?;

    let mut files: Vec<SkillResource> = entries
        .iter()
        .filter(|entry| entry.kind == Kind::File)
        .map(|entry| SkillResource::from_bytes(uri_of(&entry.path), &bytes_of[entry.path.as_str()]))
        .collect();
    files.sort_by(|a, b| a.uri.cmp(&b.uri));

    check_limits(files.len(), files.iter().map(|file| file.size).sum())?;

    let uri = uri_of(&manifest_path);
    let listing = Resource::new(&uri, &frontmatter.name)
        .with_description(&frontmatter.description)
        .with_mime_type(MARKDOWN_MIME_TYPE)
        .with_size(manifest.len() as u64);
    let skill = Skill::new(uri, frontmatter, SkillResources::Files(files));
    skill.validate().map_err(|error| error.to_string())?;
    Ok((skill, listing))
}

/// Parse the YAML block between the leading `---` lines of a `SKILL.md`.
fn parse_frontmatter(bytes: &[u8]) -> Result<SkillFrontmatter, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "SKILL.md is not UTF-8".to_string())?;
    let mut lines = text.split_inclusive('\n');
    if lines.next().map(str::trim_end) != Some("---") {
        return Err("SKILL.md does not start with a --- frontmatter block".into());
    }
    let start = text.find('\n').map_or(text.len(), |index| index + 1);
    let mut end = start;
    for line in lines {
        if line.trim_end() == "---" {
            return yaml_serde::from_str(&text[start..end])
                .map_err(|error| format!("invalid SKILL.md frontmatter: {error}"));
        }
        end += line.len();
    }
    Err("SKILL.md frontmatter has no closing ---".into())
}

/// Every entry below `root` except hidden ones, depth first, by name.
fn walk(root: &Path, handle: &Dir) -> Result<Vec<Entry>, SkillCatalogError> {
    fn visit(
        root: &Path,
        handle: &Dir,
        dir: &str,
        tree: &mut Vec<Entry>,
    ) -> Result<(), SkillCatalogError> {
        let io_error = |source| SkillCatalogError::Io {
            path: root.join(dir),
            source,
        };
        let mut entries = handle
            .read_dir(if dir.is_empty() { "." } else { dir })
            .map_err(io_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(io_error)?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name();
            let (name, utf8) = match name.into_string() {
                Ok(name) => (name, true),
                Err(name) => (name.to_string_lossy().into_owned(), false),
            };
            if name.starts_with('.') {
                continue;
            }
            // `DirEntry::file_type` does not follow symlinks.
            let file_type = entry.file_type().map_err(io_error)?;
            let kind = match (utf8, file_type.is_file(), file_type.is_dir()) {
                (true, true, _) => Kind::File,
                (true, _, true) => Kind::Directory,
                _ => Kind::Unsupported,
            };
            let path = if dir.is_empty() {
                name
            } else {
                format!("{dir}/{name}")
            };
            if kind == Kind::Directory {
                tree.push(Entry {
                    path: path.clone(),
                    kind,
                    size: 0,
                });
                visit(root, handle, &path, tree)?;
            } else {
                let size = match kind {
                    Kind::File => entry.metadata().map_err(io_error)?.len(),
                    _ => 0,
                };
                tree.push(Entry { path, kind, size });
            }
        }
        Ok(())
    }

    let mut tree = Vec::new();
    visit(root, handle, "", &mut tree)?;
    Ok(tree)
}

/// Any non-directory named `SKILL.md`, so a symlinked one is reported, not ignored.
fn is_manifest(entry: &Entry) -> bool {
    entry.kind != Kind::Directory && file_name(&entry.path) == SKILL_MD
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

fn file_name(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, name)| name)
}

fn uri_of(path: &str) -> String {
    let mut uri = String::from(URI_SCHEME);
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

/// Whether files of this type are sent as `text` rather than a base64 `blob`.
fn is_textual(mime_type: &str) -> bool {
    mime_type.starts_with("text/")
        || matches!(
            mime_type,
            "application/json"
                | "application/xml"
                | "application/yaml"
                | "application/toml"
                | "application/x-sh"
                | "image/svg+xml"
        )
}

fn mime_type_of(path: &str) -> &'static str {
    let extension = file_name(path)
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase());
    match extension.as_deref() {
        Some("md" | "markdown") => MARKDOWN_MIME_TYPE,
        Some("txt") => "text/plain",
        Some("html" | "htm") => "text/html",
        Some("css") => "text/css",
        Some("csv") => "text/csv",
        Some("xml") => "application/xml",
        Some("json") => "application/json",
        Some("yaml" | "yml") => "application/yaml",
        Some("toml") => "application/toml",
        Some("js" | "mjs") => "text/javascript",
        Some("ts") => "text/x-typescript",
        Some("py") => "text/x-python",
        Some("rs") => "text/x-rust",
        Some("sh") => "application/x-sh",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("webp") => "image/webp",
        Some("pdf") => "application/pdf",
        Some("zip") => "application/zip",
        _ => "application/octet-stream",
    }
}
