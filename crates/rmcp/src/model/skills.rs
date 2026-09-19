// SEP-2640: Skills Extension — data types, request/response types, and URI utilities.
//
// Implements the normative parts of
// https://github.com/modelcontextprotocol/modelcontextprotocol/pull/2640
//
// Skills are transported as MCP Resources. See §Resource Mapping and §Discovery of
// the spec for the exact wire shapes.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::{
    ConstString, MetaObject, PaginatedRequestParams, Request, RequestMetaObject,
    RequestOptionalParam, ResultType,
};
use crate::const_string;

// =============================================================================
// Skill entry
// =============================================================================

/// A skill entry returned by `skills/list` or `skills/get`.
///
/// Mirrors the spec's JSON shape (see §Enumeration via `skills/list` in SEP-2640):
///
/// ```json
/// {
///   "uri": "skill://doc-workflow/SKILL.md",
///   "frontmatter": { "name": "doc-workflow", "description": "..." },
///   "resources": [
///     { "uri": "skill://doc-workflow/SKILL.md", "digest": "sha256:...", "size": 1234 }
///   ],
///   "_meta": { "ttlMs": 600000, "cacheScope": "metatarsal" }
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SkillEntry {
    /// Resource URI of the skill's `SKILL.md`, e.g. `skill://doc-workflow/SKILL.md`.
    pub uri: String,

    /// Verbatim copy of the `SKILL.md` YAML frontmatter, rendered as JSON.
    ///
    /// The spec calls this "the frontmatter properties, rendered as JSON".
    /// Required keys: `name` (MUST equal the final segment of the skill path),
    /// `description`. Optional: `version`, `license`, plus any custom metadata.
    pub frontmatter: serde_json::Value,

    /// Optional array of the skill's files with their URIs, SHA-256 digests,
    /// and sizes, or the literal string `"dynamic"` for dynamically-generated
    /// skills. Omitted from bare `SKILL.md`-only skills.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<SkillResources>,

    /// Optional protocol-level metadata, including SEP-2549 caching fields.
    /// REQUIRED on `skills/list` results in protocol version 2026-07-28+
    /// (SEP-2549); not required on `skills/get` results.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<MetaObject>,
}

impl SkillEntry {
    pub fn new(uri: impl Into<String>, frontmatter: serde_json::Value) -> Self {
        Self {
            uri: uri.into(),
            frontmatter,
            resources: None,
            meta: None,
        }
    }
}

/// The `resources` field of a skill entry.
///
/// The spec allows either a concrete file list or the literal `"dynamic"`.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub enum SkillResources {
    /// Fixed list of file entries with digests and sizes.
    FileList(Vec<SkillResource>),
    /// Dynamic skill whose content is generated on the fly.
    /// The wire form is the bare JSON string `"dynamic"`.
    Dynamic,
}

impl Serialize for SkillResources {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::FileList(files) => files.serialize(serializer),
            Self::Dynamic => serializer.serialize_str("dynamic"),
        }
    }
}

impl<'de> Deserialize<'de> for SkillResources {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::{self, MapAccess, SeqAccess, Visitor};
        struct SkillResourcesVisitor;
        impl<'de> Visitor<'de> for SkillResourcesVisitor {
            type Value = SkillResources;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a skill resources list or the string \"dynamic\"")
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                if value == "dynamic" {
                    Ok(SkillResources::Dynamic)
                } else {
                    Err(de::Error::invalid_type(de::Unexpected::Str(value), &self))
                }
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut files = Vec::new();
                while let Some(file) = seq.next_element::<SkillResource>()? {
                    files.push(file);
                }
                Ok(SkillResources::FileList(files))
            }
            fn visit_map<A: MapAccess<'de>>(self, _map: A) -> Result<Self::Value, A::Error> {
                Err(de::Error::invalid_type(de::Unexpected::Map, &self))
            }
        }
        deserializer.deserialize_any(SkillResourcesVisitor)
    }
}

impl SkillResources {
    pub fn is_dynamic(&self) -> bool {
        matches!(self, Self::Dynamic)
    }

    pub fn as_files(&self) -> Option<&[SkillResource]> {
        match self {
            Self::FileList(files) => Some(files),
            Self::Dynamic => None,
        }
    }
}

/// A single file inside a skill, with its URI, SHA-256 digest, and size.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SkillResource {
    pub uri: String,
    pub digest: String,
    pub size: u64,
}

impl SkillResource {
    pub fn new(uri: impl Into<String>, digest: impl Into<String>, size: u64) -> Self {
        Self {
            uri: uri.into(),
            digest: digest.into(),
            size,
        }
    }
}

// =============================================================================
// Request / response types
// =============================================================================

const_string!(SkillsListRequestMethod = "skills/list");

/// Request to list the skills published by a server.
///
/// The request carries an optional pagination cursor (no required params).
pub type SkillsListRequest = RequestOptionalParam<SkillsListRequestMethod, PaginatedRequestParams>;

/// Response to `skills/list`.
///
/// Carries the skill entries for this page plus (for protocol 2026-07-28+)
/// the SEP-2549 caching fields (`ttlMs`/`cacheScope`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SkillsListResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_type: Option<ResultType>,
    pub skills: Vec<SkillEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_scope: Option<String>,
}

impl Default for SkillsListResult {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl SkillsListResult {
    pub fn new(skills: Vec<SkillEntry>) -> Self {
        Self {
            result_type: Some(ResultType::COMPLETE),
            skills,
            next_cursor: None,
            ttl_ms: None,
            cache_scope: None,
        }
    }
}

const_string!(SkillsGetRequestMethod = "skills/get");

/// Parameters for retrieving a single skill by URI.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SkillsGetRequestParams {
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<RequestMetaObject>,
    pub uri: String,
}

impl SkillsGetRequestParams {
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            meta: None,
            uri: uri.into(),
        }
    }
}

impl crate::model::RequestParamsMeta for SkillsGetRequestParams {
    fn meta(&self) -> Option<&RequestMetaObject> {
        self.meta.as_ref()
    }
    fn meta_mut(&mut self) -> &mut Option<RequestMetaObject> {
        &mut self.meta
    }
}

pub type SkillsGetRequest = Request<SkillsGetRequestMethod, SkillsGetRequestParams>;

/// Response to `skills/get`.
///
/// Carries a single skill entry. No pagination cursor (a single entry is not
/// a list). The caching fields (`ttlMs`/`cacheScope`) are left open — the
/// spec does not settle whether `skills/get` results carry them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SkillsGetResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_type: Option<ResultType>,
    pub skill: SkillEntry,
}

impl SkillsGetResult {
    pub fn new(skill: SkillEntry) -> Self {
        Self {
            result_type: Some(ResultType::COMPLETE),
            skill,
        }
    }
}

// =============================================================================
// Directory listing (resources/directory/read for skill:// URIs)
// =============================================================================

const_string!(ResourcesDirectoryReadRequestMethod = "resources/directory/read");

/// Parameters for reading a directory resource.
///
/// The request carries the directory URI and an optional pagination cursor.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ResourcesDirectoryReadRequestParams {
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<RequestMetaObject>,
    pub uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

impl ResourcesDirectoryReadRequestParams {
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            meta: None,
            uri: uri.into(),
            cursor: None,
        }
    }
}

impl crate::model::RequestParamsMeta for ResourcesDirectoryReadRequestParams {
    fn meta(&self) -> Option<&RequestMetaObject> {
        self.meta.as_ref()
    }
    fn meta_mut(&mut self) -> &mut Option<RequestMetaObject> {
        &mut self.meta
    }
}

pub type ResourcesDirectoryReadRequest =
    Request<ResourcesDirectoryReadRequestMethod, ResourcesDirectoryReadRequestParams>;

/// Response listing the children of a directory.
///
/// Carries the `resultType` discriminator (SEP-2322) so it can be stripped for
/// legacy peers the same way as other resource list results.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ResourcesDirectoryReadResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_type: Option<ResultType>,
    pub children: Vec<DirectoryEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_scope: Option<String>,
}

impl ResourcesDirectoryReadResult {
    pub fn new(children: Vec<DirectoryEntry>) -> Self {
        Self {
            result_type: Some(ResultType::COMPLETE),
            children,
            next_cursor: None,
            ttl_ms: None,
            cache_scope: None,
        }
    }
}

/// A single entry in a directory listing.
///
/// The spec's `resources/directory/read` result carries the same `Resource`-shaped
/// objects as `resources/list`, but in practice skill directory listings only need
/// URI, name, and a directory flag. We model the minimal shape here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct DirectoryEntry {
    pub uri: String,
    /// Whether this entry is a directory (`inode/directory`).
    pub is_directory: bool,
    /// The entry's name — the final segment of its URI.
    pub name: String,
}

impl DirectoryEntry {
    pub fn file(uri: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            is_directory: false,
            name: name.into(),
        }
    }

    pub fn dir(uri: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            is_directory: true,
            name: name.into(),
        }
    }
}

// =============================================================================
// URI parsing helpers
// =============================================================================

/// Parse a `skill://` URI into `(skill_path, file_path)`.
///
/// Per the spec: `skill://<skill-path>/<file-path>`.
///
/// - `<skill-path>` MAY be single-segment (`git-workflow`) or nested
///   (`acme/billing/refunds`).
/// - For `skill://git-workflow/SKILL.md` the file-path is `SKILL.md` and the
///   skill-path is `git-workflow`.
/// - Directory URIs are written without a trailing slash and without a file-path:
///   `skill://git-workflow`.
///
/// Returns `None` for URIs that are not valid `skill://` skill/file URIs.
pub fn parse_skill_uri(uri: &str) -> Option<(String, String)> {
    let stripped = uri.strip_prefix("skill://")?;
    if stripped.is_empty() {
        return None;
    }
    // Reject trailing slashes — directory URIs are written without them
    if stripped.ends_with('/') {
        return None;
    }
    // Split on the LAST `/` — skill path may be nested (acme/billing/refunds)
    // but the file path is always the final segment
    let (skill_path, file_path) = stripped.rsplit_once('/')?;
    if skill_path.is_empty() || file_path.is_empty() {
        return None;
    }
    Some((skill_path.to_string(), file_path.to_string()))
}

/// Extract the skill `<skill-path>` from a `skill://` URI.
///
/// Returns `None` if the URI is not a valid skill URI or is a directory URI
/// without a file component.
pub fn skill_path_from_uri(uri: &str) -> Option<String> {
    let (path, file) = parse_skill_uri(uri)?;
    if file.is_empty() {
        // Directory URI — still has a skill path
        return Some(path);
    }
    Some(path)
}

/// Extract the skill name (final segment of `<skill-path>`) from a `skill://` URI.
pub fn skill_name_from_uri(uri: &str) -> Option<String> {
    let (path, _file) = parse_skill_uri(uri)?;
    let name = path.rsplit('/').next()?;
    if name.is_empty() {
        return None;
    }
    Some(name.to_string())
}

/// Validate that the `name` field of the frontmatter matches the final segment
/// of the skill path, per the spec's resource-mapping constraint.
pub fn validate_skill_name(uri: &str, frontmatter: &serde_json::Value) -> Result<(), String> {
    let expected = skill_name_from_uri(uri).ok_or_else(|| format!("invalid skill URI: {uri}"))?;
    let actual = frontmatter
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "frontmatter missing required 'name' field".to_string())?;
    if actual != expected {
        return Err(format!(
            "frontmatter name '{actual}' does not match skill path final segment '{expected}'"
        ));
    }
    Ok(())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn skill_entry_round_trip() {
        let entry = SkillEntry {
            uri: "skill://doc-workflow/SKILL.md".to_string(),
            frontmatter: json!({"name": "doc-workflow", "description": "Follow this team's Git conventions"}),
            resources: Some(SkillResources::FileList(vec![SkillResource::new(
                "skill://doc-workflow/references/APPENDIX.md",
                "sha256:a1b2c3d4e5f6789012345678901234567890abcd",
                1234,
            )])),
            meta: Some(MetaObject::new()),
        };
        let json = serde_json::to_string_pretty(&entry).unwrap();
        let decoded: SkillEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, decoded);
    }

    #[test]
    fn skills_list_response_round_trip() {
        let list = SkillsListResult::new(vec![SkillEntry::new(
            "skill://git-workflow/SKILL.md",
            json!({"name": "git-workflow", "description": "Git conventions"}),
        )]);
        let json = serde_json::to_string_pretty(&list).unwrap();
        let decoded: SkillsListResult = serde_json::from_str(&json).unwrap();
        assert_eq!(list, decoded);
    }

    #[test]
    fn skill_with_dynamic_resources() {
        let entry = SkillEntry {
            uri: "skill://dynamic-skill/SKILL.md".to_string(),
            frontmatter: json!({"name": "dynamic-skill", "description": "Dynamic skill"}),
            resources: Some(SkillResources::Dynamic),
            meta: None,
        };
        let json = serde_json::to_string_pretty(&entry).unwrap();
        let decoded: SkillEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, decoded);
        assert!(decoded.resources.unwrap().is_dynamic());
    }

    #[test]
    fn parse_bare_skill_uri() {
        let (path, file) = parse_skill_uri("skill://git-workflow/SKILL.md").unwrap();
        assert_eq!(path, "git-workflow");
        assert_eq!(file, "SKILL.md");
    }

    #[test]
    fn parse_nested_skill_uri() {
        let (path, file) = parse_skill_uri("skill://acme/billing/refunds/SKILL.md").unwrap();
        assert_eq!(path, "acme/billing/refunds");
        assert_eq!(file, "SKILL.md");
    }

    #[test]
    fn parse_subfile_uri() {
        // Per spec: skill path is everything before the last `/`, file path is the last segment
        let (path, file) = parse_skill_uri("skill://pdf-processing/references/FORMS.md").unwrap();
        assert_eq!(path, "pdf-processing/references");
        assert_eq!(file, "FORMS.md");
    }

    #[test]
    fn parse_non_skill_uri() {
        assert!(parse_skill_uri("https://example.com/SKILL.md").is_none());
        assert!(parse_skill_uri("file:///skills/SKILL.md").is_none());
        assert!(parse_skill_uri("skill://").is_none());
        assert!(parse_skill_uri("skill:///SKILL.md").is_none());
    }

    #[test]
    fn parse_trailing_slash_directory() {
        // `skill://git-workflow/` — trailing slash after skill-path, no file
        // This is NOT a valid directory URI (the spec writes directory URIs without
        // trailing slash). We treat it as invalid to match the spec's wire shape.
        assert!(parse_skill_uri("skill://git-workflow/").is_none());
    }

    #[test]
    fn validate_skill_name_mismatch() {
        let uri = "skill://git-workflow/SKILL.md";
        let frontmatter = json!({"name": "wrong-name", "description": "..."});
        let err = validate_skill_name(uri, &frontmatter).unwrap_err();
        assert!(err.contains("wrong-name"));
        assert!(err.contains("git-workflow"));
    }

    #[test]
    fn validate_skill_name_matches() {
        let uri = "skill://git-workflow/SKILL.md";
        let frontmatter = json!({"name": "git-workflow", "description": "..."});
        assert!(validate_skill_name(uri, &frontmatter).is_ok());
    }

    #[test]
    fn resources_directory_read_round_trip() {
        let resp = ResourcesDirectoryReadResult::new(vec![
            DirectoryEntry::file("skill://a/SKILL.md", "SKILL.md"),
            DirectoryEntry::dir("skill://b/", "b"),
        ]);
        let json = serde_json::to_string_pretty(&resp).unwrap();
        let decoded: ResourcesDirectoryReadResult = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, decoded);
    }
}
