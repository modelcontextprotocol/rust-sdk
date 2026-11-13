//! Skills served from a directory tree (SEP-2640).
//!
//! Each folder below the root that holds a `SKILL.md` is a skill, served at
//! `skill://<path from the root>/SKILL.md`. A folder of skills can nest other
//! skills: the nested skill is listed on its own, and its files also appear
//! in the manifest of every skill above it. Hidden entries, whose names
//! start with `.`, are neither listed nor served.
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
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError, RwLock},
};

use base64::{Engine, prelude::BASE64_STANDARD};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};

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
/// File and directory paths are opened beneath the root without following
/// symlinks. If an entry disappears or changes to an unsupported type when
/// it is opened, the catalog skips the skills that include it and refreshes
/// the others. A hard link inside the root is an ordinary file entry and can
/// share its contents with a name outside the root.
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
        let text = file
            .mime_type
            .is_some_and(is_textual)
            .then(|| std::str::from_utf8(&file.bytes).ok())
            .flatten();
        let contents = match text {
            Some(text) => ResourceContents::text(text, uri),
            None => ResourceContents::blob(BASE64_STANDARD.encode(&file.bytes), uri),
        };
        let contents = match file.mime_type {
            Some(mime_type) => contents.with_mime_type(mime_type),
            None => contents,
        };
        Some(ReadResourceResult::new(vec![contents]))
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
    mime_type: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    File,
    Directory,
    /// An entry that disappeared, changed type, or cannot be served.
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
        Self::from_walk(root, &handle, &tree)
    }

    fn from_walk(root: &Path, handle: &Dir, tree: &[Entry]) -> Result<Self, SkillCatalogError> {
        let skill_dirs: BTreeSet<&str> = tree
            .iter()
            .filter(|entry| is_manifest(entry))
            .map(|entry| parent(&entry.path))
            .collect();

        let mut inside: BTreeMap<&str, Vec<&Entry>> = BTreeMap::new();
        for entry in tree {
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
                tracing::warn!(path = %entry.path, "skipping an unsupported or unavailable entry");
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

        let mut cache = FileCache::default();
        let mut within_limit: Vec<(&str, &[&Entry])> = Vec::new();
        for (dir, entries) in kept {
            match read_within_limit(handle, root, entries, &mut cache)? {
                SkillRead::Ready => within_limit.push((dir, entries)),
                SkillRead::Changed(path) => tracing::warn!(
                    skill = %dir, %path, "skipping skill: entry changed after the walk"
                ),
                SkillRead::OverLimit => tracing::warn!(
                    skill = %dir,
                    "skipping skill: grew past {SKILL_BYTE_LIMIT} bytes after the walk"
                ),
            }
        }

        let bytes_of = cache.bytes;
        let mut snapshot = Snapshot::default();
        let mut directories: BTreeSet<&str> = BTreeSet::new();
        for (dir, entries) in within_limit {
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
        for entry in tree {
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
            let mut resource = Resource::new(uri, name).with_size(file.bytes.len() as u64);
            resource.mime_type = file.mime_type.map(Into::into);
            Some(resource)
        }
        Kind::Unsupported => None,
    }
}

/// Rules that the walk alone can check: no special files, and within the limits.
fn check_walked(entries: &[&Entry]) -> Result<(), String> {
    if let Some(entry) = entries.iter().find(|entry| entry.kind == Kind::Unsupported) {
        return Err(format!(
            "{} is unavailable, is a symlink or special file, or has a non-UTF-8 name",
            entry.path
        ));
    }
    let files = entries.iter().filter(|entry| entry.kind == Kind::File);
    let (files, bytes) = (
        files.clone().count(),
        files.map(|entry| entry.size).sum::<u64>(),
    );
    if files > SKILL_FILE_LIMIT || bytes > SKILL_BYTE_LIMIT {
        return Err(format!(
            "{files} files and {bytes} bytes exceed the limit of {SKILL_FILE_LIMIT} files or \
             {SKILL_BYTE_LIMIT} bytes, so hosts need not load it"
        ));
    }
    Ok(())
}

#[derive(Default)]
struct FileCache<'a> {
    bytes: HashMap<&'a str, Arc<[u8]>>,
    /// All enclosing skills must agree when the same walked file became unavailable.
    changed: HashSet<&'a str>,
}

#[derive(Debug, PartialEq, Eq)]
enum SkillRead<'a> {
    Ready,
    OverLimit,
    Changed(&'a str),
}

/// Share complete file reads between nested skills, charging each skill its own byte budget.
fn read_within_limit<'a>(
    handle: &Dir,
    root: &Path,
    entries: &[&'a Entry],
    cache: &mut FileCache<'a>,
) -> Result<SkillRead<'a>, SkillCatalogError> {
    let mut budget = SKILL_BYTE_LIMIT + 1;
    for entry in entries.iter().filter(|entry| entry.kind == Kind::File) {
        let path = entry.path.as_str();
        if cache.changed.contains(path) {
            return Ok(SkillRead::Changed(path));
        }
        let len = match cache.bytes.get(path) {
            Some(bytes) => bytes.len() as u64,
            None => {
                let bytes = match read_regular_file(handle, path, budget) {
                    Ok(bytes) => bytes,
                    Err(error) if entry_changed(&error) => {
                        cache.changed.insert(path);
                        return Ok(SkillRead::Changed(path));
                    }
                    Err(source) => {
                        return Err(SkillCatalogError::Io {
                            path: root.join(path),
                            source,
                        });
                    }
                };
                let len = bytes.len() as u64;
                if len < budget {
                    cache.bytes.insert(path, bytes.into());
                }
                len
            }
        };
        budget = budget.saturating_sub(len);
        if budget == 0 {
            return Ok(SkillRead::OverLimit);
        }
    }
    Ok(SkillRead::Ready)
}

/// Errors a no-follow open can report when a walked component disappears or changes type.
/// cap-std passes through platform-specific errors for a refused symlink.
fn entry_changed(error: &io::Error) -> bool {
    if matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory | io::ErrorKind::IsADirectory
    ) {
        return true;
    }
    match error.raw_os_error() {
        #[cfg(unix)]
        Some(libc::ELOOP) => true,
        #[cfg(any(target_os = "freebsd", target_os = "dragonfly"))]
        Some(libc::EMLINK) => true,
        #[cfg(target_os = "netbsd")]
        Some(libc::EFTYPE) => true,
        #[cfg(windows)]
        Some(code) if code as u32 == windows_sys::Win32::Foundation::ERROR_STOPPED_ON_SYMLINK => {
            true
        }
        _ => false,
    }
}

fn read_regular_file(root: &Dir, path: &str, budget: u64) -> io::Result<Vec<u8>> {
    if !path
        .split('/')
        .all(|part| Path::new(part).file_name() == Some(part.as_ref()))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected normal relative path components",
        ));
    }
    // No-follow applies to the last component of an open. Open each directory
    // separately so an ancestor replaced since the walk cannot redirect the read.
    let mut dir = root;
    let mut child;
    for component in parent(path).split_terminator('/') {
        child = dir.open_dir_nofollow(component)?;
        dir = &child;
    }
    let mut options = OpenOptions::new();
    // Open nonblocking so we can reject a substituted FIFO before reading it.
    options.read(true).follow(FollowSymlinks::No).nonblock(true);
    let name = file_name(path);
    let file = dir.open_with(name, &options).map_err(|error| {
        // Some systems reject a special file at open time. Inspect only the
        // entry type to classify that error; all reads use the opened handle.
        let check_type = match error.raw_os_error() {
            #[cfg(unix)]
            Some(libc::ENXIO | libc::ENODEV) => true,
            #[cfg(windows)]
            Some(code)
                if code as u32 == windows_sys::Win32::Foundation::ERROR_DIRECTORY_NOT_SUPPORTED =>
            {
                true
            }
            _ => matches!(
                error.kind(),
                io::ErrorKind::PermissionDenied | io::ErrorKind::Unsupported
            ),
        };
        if check_type && dir.symlink_metadata(name).is_ok_and(|m| !m.is_file()) {
            io::Error::new(io::ErrorKind::NotFound, "walked regular file was replaced")
        } else {
            error
        }
    })?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "walked regular file was replaced",
        ));
    }
    let mut bytes = Vec::new();
    file.take(budget).read_to_end(&mut bytes)?;
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
            .entries()
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
            let path = if dir.is_empty() {
                name
            } else {
                format!("{dir}/{name}")
            };
            // DirEntry metadata describes the entry itself, including a symlink.
            let (kind, size) = match entry.metadata() {
                Ok(m) if utf8 && m.is_file() => (Kind::File, m.len()),
                Ok(m) if utf8 && m.is_dir() => (Kind::Directory, 0),
                Ok(_) => (Kind::Unsupported, 0),
                Err(error) if entry_changed(&error) => (Kind::Unsupported, 0),
                Err(source) => {
                    return Err(SkillCatalogError::Io {
                        path: root.join(&path),
                        source,
                    });
                }
            };
            if kind == Kind::Directory {
                let index = tree.len();
                tree.push(Entry {
                    path: path.clone(),
                    kind,
                    size: 0,
                });
                let result = handle
                    .open_dir_nofollow(entry.file_name())
                    .map_err(|source| SkillCatalogError::Io {
                        path: root.join(&path),
                        source,
                    })
                    .and_then(|child| visit(root, &child, &path, tree));
                match result {
                    Err(SkillCatalogError::Io { source, .. }) if entry_changed(&source) => {
                        // An incomplete subtree invalidates every enclosing skill.
                        tree.truncate(index + 1);
                        tree[index].kind = Kind::Unsupported;
                    }
                    other => other?,
                }
            } else {
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

/// `None` for an unknown extension, so `mimeType` is omitted.
fn mime_type_of(path: &str) -> Option<&'static str> {
    let extension = file_name(path)
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase());
    let mime_type = match extension.as_deref() {
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
        _ => return None,
    };
    Some(mime_type)
}

#[cfg(test)]
mod tests {
    use std::fs::File;

    use super::*;

    #[test]
    fn read_within_limit_should_stop_at_the_budget_and_not_cache_the_truncated_file() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a", "b"] {
            File::create(dir.path().join(name))
                .unwrap()
                .set_len(SKILL_BYTE_LIMIT)
                .unwrap();
        }
        // Sizes the walk recorded before the files grew.
        let stale = |name: &str| Entry {
            path: name.to_string(),
            kind: Kind::File,
            size: 1,
        };
        let (a, b) = (stale("a"), stale("b"));
        let handle = Dir::open_ambient_dir(dir.path(), ambient_authority()).unwrap();
        let mut cache = FileCache::default();

        let outcome = read_within_limit(&handle, dir.path(), &[&a, &b], &mut cache).unwrap();

        assert_eq!(outcome, SkillRead::OverLimit);
        assert_eq!(cache.bytes.keys().collect::<Vec<_>>(), vec![&"a"]);
    }

    #[cfg(unix)]
    #[test]
    fn reads_reject_file_and_directory_symlinks_installed_after_the_walk() {
        use std::{fs, os::unix::fs::symlink};

        let root = tempfile::tempdir().unwrap();
        let private = root.path().join(".private");
        let public = root.path().join("public");
        fs::create_dir(&private).unwrap();
        fs::create_dir_all(public.join("assets")).unwrap();
        fs::write(private.join("notes.txt"), "private").unwrap();
        fs::write(public.join("notes.txt"), "public").unwrap();
        fs::write(public.join("assets/notes.txt"), "public").unwrap();

        let handle = Dir::open_ambient_dir(root.path(), ambient_authority()).unwrap();
        let tree = walk(root.path(), &handle).unwrap();
        fs::remove_file(public.join("notes.txt")).unwrap();
        symlink("../.private/notes.txt", public.join("notes.txt")).unwrap();
        fs::rename(public.join("assets"), public.join("old-assets")).unwrap();
        symlink("../.private", public.join("assets")).unwrap();

        for path in ["public/notes.txt", "public/assets/notes.txt"] {
            let entry = tree.iter().find(|entry| entry.path == path).unwrap();
            assert_eq!(entry.kind, Kind::File);
            let mut cache = FileCache::default();
            assert_eq!(
                read_within_limit(&handle, root.path(), &[entry], &mut cache).unwrap(),
                SkillRead::Changed(path),
            );
            assert!(cache.bytes.is_empty());
        }
    }

    #[cfg(unix)]
    #[test]
    fn rebuilt_snapshot_skips_affected_skills_and_refreshes_healthy_siblings() {
        use std::{fs, os::unix::fs::symlink};

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        for dir in ["parent", "parent/child", "healthy"] {
            let path = root.path().join(dir);
            fs::create_dir_all(&path).unwrap();
            fs::write(
                path.join(SKILL_MD),
                format!("---\nname: {}\ndescription: Example\n---\n", file_name(dir)),
            )
            .unwrap();
        }
        let asset = root.path().join("parent/child/asset.txt");
        let sibling = root.path().join("healthy/news.txt");
        fs::write(&asset, "old asset").unwrap();
        fs::write(&sibling, "old news").unwrap();
        let before = Snapshot::load(root.path()).unwrap();
        assert_eq!(before.skills.len(), 3);

        let handle = Dir::open_ambient_dir(root.path(), ambient_authority()).unwrap();
        let tree = walk(root.path(), &handle).unwrap();
        fs::write(outside.path().join("asset.txt"), "outside").unwrap();
        fs::remove_file(&asset).unwrap();
        symlink(outside.path().join("asset.txt"), &asset).unwrap();
        fs::write(sibling, "updated news").unwrap();
        let after = Snapshot::from_walk(root.path(), &handle, &tree).unwrap();

        assert_eq!(
            after.skills.iter().map(Skill::name).collect::<Vec<_>>(),
            ["healthy"]
        );
        assert!(!after.files.contains_key("skill://parent/child/asset.txt"));
        assert_eq!(
            after.files["skill://healthy/news.txt"].bytes.as_ref(),
            b"updated news"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_substituted_fifo_without_waiting_for_a_writer() {
        use std::{fs, process::Command, sync::mpsc, thread, time::Duration};

        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("asset.txt");
        fs::write(&path, "original").unwrap();
        let handle = Dir::open_ambient_dir(root.path(), ambient_authority()).unwrap();
        let tree = walk(root.path(), &handle).unwrap();
        assert_eq!(tree[0].kind, Kind::File);
        fs::remove_file(&path).unwrap();
        assert!(Command::new("mkfifo").arg(path).status().unwrap().success());

        let (done, result) = mpsc::channel();
        let worker = thread::spawn(move || {
            let error = read_regular_file(&handle, &tree[0].path, SKILL_BYTE_LIMIT).unwrap_err();
            let _ = done.send(entry_changed(&error));
        });
        assert!(
            result
                .recv_timeout(Duration::from_secs(5))
                .expect("opening a FIFO waited for a writer")
        );
        worker.join().unwrap();
    }

    #[test]
    fn a_missing_file_stays_unavailable_to_overlapping_skills_in_one_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let handle = Dir::open_ambient_dir(root.path(), ambient_authority()).unwrap();
        let file = Entry {
            path: "shared.txt".into(),
            kind: Kind::File,
            size: 1,
        };
        let mut cache = FileCache::default();
        let mut read = || read_within_limit(&handle, root.path(), &[&file], &mut cache).unwrap();
        assert_eq!(read(), SkillRead::Changed("shared.txt"));
        std::fs::write(root.path().join(&file.path), "restored").unwrap();
        assert_eq!(read(), SkillRead::Changed("shared.txt"));
        assert!(cache.bytes.is_empty());
    }

    #[test]
    fn rejects_malformed_walked_paths() {
        let root = tempfile::tempdir().unwrap();
        let handle = Dir::open_ambient_dir(root.path(), ambient_authority()).unwrap();
        for path in [
            "",
            ".",
            "..",
            "/absolute",
            "sub//file",
            "sub/../file",
            "sub/./file",
            "sub/",
        ] {
            let error = read_regular_file(&handle, path, SKILL_BYTE_LIMIT).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{path:?}");
        }
    }
}
