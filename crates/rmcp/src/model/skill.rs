//! Skill types for the MCP Skills extension (SEP-2640).
//!
//! A skill is a directory of files served over the Resources primitive:
//! `skills/list` and `skills/get` return the manifest of each skill, and
//! `resources/read` serves the file bytes. The request and result wiring
//! lives in the parent module next to the other methods.

#[cfg(feature = "schemars")]
use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::{
    CacheScope, JsonObject, MetaObject, Resource, ResultType, deserialize_cache_scope,
    deserialize_ttl_ms,
};

/// Extension identifier for the MCP Skills extension (SEP-2640).
pub const SKILLS_EXTENSION_ID: &str = "io.modelcontextprotocol/skills";

/// MIME type that marks a resource as a directory.
pub const DIRECTORY_MIME_TYPE: &str = "inode/directory";

impl Resource {
    /// Returns `true` when the resource is a directory.
    pub fn is_directory(&self) -> bool {
        self.mime_type.as_deref() == Some(DIRECTORY_MIME_TYPE)
    }
}

/// Settings of the skills extension capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct SkillsCapability {
    /// The server serves `resources/directory/read`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub directory_read: bool,
}

impl SkillsCapability {
    /// Capability without directory reads.
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare `resources/directory/read` support.
    pub fn with_directory_read(mut self) -> Self {
        self.directory_read = true;
        self
    }
}

fn is_false(value: &bool) -> bool {
    !value
}

/// One file of a skill, with the digest and size of its bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct SkillResource {
    /// Resource URI of the file.
    pub uri: String,
    /// `sha256:` followed by 64 lowercase hex characters.
    pub digest: String,
    /// Length of the file in bytes.
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

    /// Describe `bytes` as the file at `uri`, with its digest and size computed.
    pub fn from_bytes(uri: impl Into<String>, bytes: &[u8]) -> Self {
        Self::new(uri, sha256_digest(bytes), bytes.len() as u64)
    }

    /// Check that `bytes` has the size and digest this entry promises.
    ///
    /// Compare the size first, so a wrong length is reported without hashing.
    /// Whether the frontmatter in the bytes matches `skills/get` is the host's
    /// check, not this one.
    pub fn verify(&self, bytes: &[u8]) -> Result<(), SkillIntegrityError> {
        let size = bytes.len() as u64;
        if size != self.size {
            return Err(SkillIntegrityError::Size {
                uri: self.uri.clone(),
                expected: self.size,
                actual: size,
            });
        }
        let digest = sha256_digest(bytes);
        if digest != self.digest {
            return Err(SkillIntegrityError::Digest {
                uri: self.uri.clone(),
                expected: self.digest.clone(),
                actual: digest,
            });
        }
        Ok(())
    }
}

/// The `sha256:` digest of `bytes` in the form the spec requires.
pub fn sha256_digest(bytes: &[u8]) -> String {
    use std::fmt::Write;

    use sha2::{Digest, Sha256};

    let hash = Sha256::digest(bytes);
    let mut digest = String::with_capacity(7 + 64);
    digest.push_str("sha256:");
    for byte in hash {
        write!(digest, "{byte:02x}").expect("writing to a String cannot fail");
    }
    digest
}

/// File bytes that do not match the manifest entry.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SkillIntegrityError {
    #[error("{uri} is {actual} bytes, the manifest says {expected}")]
    Size {
        uri: String,
        expected: u64,
        actual: u64,
    },
    #[error("{uri} has digest {actual}, the manifest says {expected}")]
    Digest {
        uri: String,
        expected: String,
        actual: String,
    },
}

/// The `SKILL.md` frontmatter, with every field the author wrote.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct SkillFrontmatter {
    pub name: String,
    pub description: String,
    /// Every frontmatter field other than `name` and `description`.
    #[serde(flatten)]
    pub extra: JsonObject,
}

impl SkillFrontmatter {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            extra: JsonObject::new(),
        }
    }

    /// Add a frontmatter field other than `name` and `description`.
    pub fn with_field(
        mut self,
        key: impl Into<String>,
        value: impl Into<serde_json::Value>,
    ) -> Self {
        self.extra.insert(key.into(), value.into());
        self
    }
}

/// The files of a skill: a full manifest, or `"dynamic"` when the content
/// changes and no stable digests exist.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillResources {
    Files(Vec<SkillResource>),
    Dynamic,
}

const DYNAMIC: &str = "dynamic";

impl Serialize for SkillResources {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Files(files) => files.serialize(serializer),
            Self::Dynamic => serializer.serialize_str(DYNAMIC),
        }
    }
}

impl<'de> Deserialize<'de> for SkillResources {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Files(Vec<SkillResource>),
            Marker(String),
        }
        match Wire::deserialize(deserializer)? {
            Wire::Files(files) => Ok(Self::Files(files)),
            Wire::Marker(marker) if marker == DYNAMIC => Ok(Self::Dynamic),
            Wire::Marker(marker) => Err(serde::de::Error::custom(format!(
                "expected a list of skill resources or \"{DYNAMIC}\", found \"{marker}\""
            ))),
        }
    }
}

#[cfg(feature = "schemars")]
impl schemars::JsonSchema for SkillResources {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("SkillResources")
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let files = generator.subschema_for::<Vec<SkillResource>>();
        schemars::json_schema!({
            "anyOf": [files, { "type": "string", "const": DYNAMIC }]
        })
    }
}

/// One skill as listed by `skills/list` or returned by `skills/get`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Skill {
    /// Resource URI of the skill's `SKILL.md`.
    pub uri: String,
    pub frontmatter: SkillFrontmatter,
    pub resources: SkillResources,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<MetaObject>,
}

impl Skill {
    pub fn new(
        uri: impl Into<String>,
        frontmatter: SkillFrontmatter,
        resources: SkillResources,
    ) -> Self {
        Self {
            uri: uri.into(),
            frontmatter,
            resources,
            meta: None,
        }
    }

    pub fn with_meta(mut self, meta: MetaObject) -> Self {
        self.meta = Some(meta);
        self
    }

    /// The skill name from the frontmatter.
    pub fn name(&self) -> &str {
        &self.frontmatter.name
    }

    /// Check the rules the spec puts on a skill entry that can be checked
    /// without reading any file.
    pub fn validate(&self) -> Result<(), SkillValidationError> {
        let Some(name) = skill_name_from_uri(&self.uri) else {
            return Err(SkillValidationError::UriNotSkillMd(self.uri.clone()));
        };
        if name != self.frontmatter.name {
            return Err(SkillValidationError::NameMismatch {
                uri: self.uri.clone(),
                name: self.frontmatter.name.clone(),
            });
        }
        let SkillResources::Files(files) = &self.resources else {
            return Ok(());
        };
        if !files.iter().any(|file| file.uri == self.uri) {
            return Err(SkillValidationError::MissingSkillMd(self.uri.clone()));
        }
        let mut seen = std::collections::HashSet::with_capacity(files.len());
        for file in files {
            if !seen.insert(file.uri.as_str()) {
                return Err(SkillValidationError::DuplicateResource(file.uri.clone()));
            }
            if !is_sha256_digest(&file.digest) {
                return Err(SkillValidationError::MalformedDigest {
                    uri: file.uri.clone(),
                    digest: file.digest.clone(),
                });
            }
        }
        Ok(())
    }
}

/// File name of the skill entry point.
pub const SKILL_MD: &str = "SKILL.md";

/// The skill name encoded in a `SKILL.md` URI: the path segment before `SKILL.md`.
pub fn skill_name_from_uri(uri: &str) -> Option<&str> {
    let path = uri.strip_suffix(SKILL_MD)?.strip_suffix('/')?;
    let name = path.rsplit('/').next()?;
    (!name.is_empty()).then_some(name)
}

/// Whether `digest` is `sha256:` followed by 64 lowercase hex characters.
pub fn is_sha256_digest(digest: &str) -> bool {
    digest.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// Check every skill of a listing, including that no URI appears twice.
pub fn validate_skills(skills: &[Skill]) -> Result<(), SkillValidationError> {
    let mut seen = std::collections::HashSet::with_capacity(skills.len());
    for skill in skills {
        skill.validate()?;
        if !seen.insert(skill.uri.as_str()) {
            return Err(SkillValidationError::DuplicateSkill(skill.uri.clone()));
        }
    }
    Ok(())
}

/// A skill entry that breaks a structural rule of the spec.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SkillValidationError {
    #[error("skill uri {0} does not end in /SKILL.md")]
    UriNotSkillMd(String),
    #[error("skill uri {uri} does not end in the frontmatter name {name}")]
    NameMismatch { uri: String, name: String },
    #[error("skill manifest {0} does not list its own SKILL.md")]
    MissingSkillMd(String),
    #[error("skill manifest lists {0} twice")]
    DuplicateResource(String),
    #[error("skill file {uri} has a malformed digest {digest}")]
    MalformedDigest { uri: String, digest: String },
    #[error("skill {0} is listed twice")]
    DuplicateSkill(String),
}

/// Result of `skills/get`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct GetSkillResult {
    /// Result type discriminator (SEP-2322); `None` on the wire means complete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_type: Option<ResultType>,
    /// Freshness in milliseconds (SEP-2549); required on 2026-07-28, absent before.
    #[serde(
        default,
        deserialize_with = "deserialize_ttl_ms",
        skip_serializing_if = "Option::is_none"
    )]
    pub ttl_ms: Option<u64>,
    /// Who may cache this result (SEP-2549); required on 2026-07-28, absent before.
    #[serde(
        default,
        deserialize_with = "deserialize_cache_scope",
        skip_serializing_if = "Option::is_none"
    )]
    pub cache_scope: Option<CacheScope>,
    pub skill: Skill,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<MetaObject>,
}

impl GetSkillResult {
    pub fn new(skill: Skill) -> Self {
        Self {
            result_type: Some(ResultType::COMPLETE),
            ttl_ms: None,
            cache_scope: None,
            skill,
            meta: None,
        }
    }

    pub fn with_ttl_ms(mut self, ttl_ms: u64) -> Self {
        self.ttl_ms = Some(ttl_ms);
        self
    }

    pub fn with_cache_scope(mut self, cache_scope: CacheScope) -> Self {
        self.cache_scope = Some(cache_scope);
        self
    }

    pub fn with_meta(mut self, meta: MetaObject) -> Self {
        self.meta = Some(meta);
        self
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn billing_skill() -> Skill {
        Skill::new(
            "skill://acme/billing/SKILL.md",
            SkillFrontmatter::new("billing", "Issue invoices"),
            SkillResources::Files(vec![SkillResource::new(
                "skill://acme/billing/SKILL.md",
                "sha256:9ff4e5c2e1a1b2c3d4e5f60718293a4b5c6d7e8f9a0b1c2d3e4f5a6b7c8d9e0f",
                1202,
            )]),
        )
    }

    mod skill_resources {
        use super::*;

        #[test]
        fn serializes_files_as_a_list() {
            let files = SkillResources::Files(vec![SkillResource::new(
                "skill://a/SKILL.md",
                "sha256:00",
                1,
            )]);
            assert_eq!(
                serde_json::to_value(&files).unwrap(),
                json!([{ "uri": "skill://a/SKILL.md", "digest": "sha256:00", "size": 1 }])
            );
        }

        #[test]
        fn serializes_dynamic_as_the_marker_string() {
            assert_eq!(
                serde_json::to_value(SkillResources::Dynamic).unwrap(),
                json!("dynamic")
            );
        }

        #[test]
        fn deserializes_the_marker_string_as_dynamic() {
            let resources: SkillResources = serde_json::from_value(json!("dynamic")).unwrap();
            assert_eq!(resources, SkillResources::Dynamic);
        }

        #[test]
        fn rejects_any_other_string() {
            let error = serde_json::from_value::<SkillResources>(json!("static")).unwrap_err();
            assert!(error.to_string().contains("found \"static\""), "{error}");
        }

        #[test]
        fn deserializes_an_empty_list_as_files() {
            let resources: SkillResources = serde_json::from_value(json!([])).unwrap();
            assert_eq!(resources, SkillResources::Files(vec![]));
        }
    }

    mod skill_frontmatter {
        use super::*;

        #[test]
        fn keeps_unknown_fields_on_round_trip() {
            let wire = json!({ "name": "billing", "description": "Issue invoices", "license": "MIT", "allowed-tools": ["Read"] });
            let frontmatter: SkillFrontmatter = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(serde_json::to_value(&frontmatter).unwrap(), wire);
        }

        #[test]
        fn requires_name() {
            let error = serde_json::from_value::<SkillFrontmatter>(json!({ "description": "x" }))
                .unwrap_err();
            assert!(error.to_string().contains("name"), "{error}");
        }
    }

    mod skill {
        use super::*;

        #[test]
        fn serializes_meta_under_underscore_meta() {
            let skill = billing_skill().with_meta(MetaObject::new());
            let value = serde_json::to_value(skill).unwrap();
            assert!(value.get("_meta").is_some(), "{value}");
        }

        #[test]
        fn name_comes_from_the_frontmatter() {
            assert_eq!(billing_skill().name(), "billing");
        }
    }

    /// The `pdf-processing` SKILL.md from the spec, whose digest and size the
    /// spec's example manifest lists.
    const SPEC_SKILL_MD: &[u8] = b"---\nname: pdf-processing\ndescription: Extract, fill, and assemble PDF documents\n---\n\n# PDF processing\n\nChoose the matching template from `templates/`.\n";
    const SPEC_SKILL_MD_DIGEST: &str =
        "sha256:99b737495721155ece826d57521e2d66141ebdc1344a400487481ea2642ab19e";

    mod sha256_digest {
        use super::*;

        #[test]
        fn matches_the_digest_in_the_spec_example() {
            assert_eq!(sha256_digest(SPEC_SKILL_MD), SPEC_SKILL_MD_DIGEST);
        }
    }

    mod skill_resource_verify {
        use super::*;

        fn spec_entry() -> SkillResource {
            SkillResource::new("skill://pdf-processing/SKILL.md", SPEC_SKILL_MD_DIGEST, 151)
        }

        #[test]
        fn accepts_the_bytes_the_manifest_describes() {
            assert_eq!(spec_entry().verify(SPEC_SKILL_MD), Ok(()));
        }

        #[test]
        fn reports_a_size_mismatch_before_hashing() {
            let error = spec_entry().verify(b"short").unwrap_err();
            assert!(
                matches!(error, SkillIntegrityError::Size { actual: 5, .. }),
                "{error}"
            );
        }

        #[test]
        fn reports_a_digest_mismatch_for_same_sized_bytes() {
            let mut tampered = SPEC_SKILL_MD.to_vec();
            tampered[0] = b'+';
            let error = spec_entry().verify(&tampered).unwrap_err();
            assert!(
                matches!(error, SkillIntegrityError::Digest { .. }),
                "{error}"
            );
        }

        #[test]
        fn from_bytes_produces_an_entry_that_verifies() {
            let entry = SkillResource::from_bytes("skill://pdf-processing/SKILL.md", SPEC_SKILL_MD);
            assert_eq!(entry, spec_entry());
        }
    }

    mod skill_name_from_uri {
        use super::*;

        #[test]
        fn returns_the_segment_before_skill_md() {
            assert_eq!(
                skill_name_from_uri("skill://acme/billing/SKILL.md"),
                Some("billing")
            );
        }

        #[test]
        fn returns_none_without_skill_md() {
            assert_eq!(skill_name_from_uri("skill://acme/billing/README.md"), None);
        }

        #[test]
        fn returns_none_for_skill_md_at_the_root() {
            assert_eq!(skill_name_from_uri("skill:///SKILL.md"), None);
        }
    }

    mod is_sha256_digest {
        use super::*;

        #[test]
        fn accepts_sixty_four_lowercase_hex_characters() {
            assert!(is_sha256_digest(&format!("sha256:{}", "a".repeat(64))));
        }

        #[test]
        fn rejects_uppercase_hex() {
            assert!(!is_sha256_digest(&format!("sha256:{}", "A".repeat(64))));
        }

        #[test]
        fn rejects_a_short_digest() {
            assert!(!is_sha256_digest("sha256:abc"));
        }

        #[test]
        fn rejects_a_missing_prefix() {
            assert!(!is_sha256_digest(&"a".repeat(64)));
        }
    }

    mod skill_validate {
        use super::*;

        #[test]
        fn accepts_a_consistent_entry() {
            assert_eq!(billing_skill().validate(), Ok(()));
        }

        #[test]
        fn accepts_a_dynamic_entry_without_files() {
            let skill = Skill::new(
                "skill://acme/billing/SKILL.md",
                SkillFrontmatter::new("billing", "Issue invoices"),
                SkillResources::Dynamic,
            );
            assert_eq!(skill.validate(), Ok(()));
        }

        #[test]
        fn rejects_a_name_that_differs_from_the_uri() {
            let mut skill = billing_skill();
            skill.frontmatter.name = "invoices".into();
            assert!(matches!(
                skill.validate(),
                Err(SkillValidationError::NameMismatch { .. })
            ));
        }

        #[test]
        fn rejects_a_manifest_without_skill_md() {
            let mut skill = billing_skill();
            skill.resources = SkillResources::Files(vec![]);
            assert!(matches!(
                skill.validate(),
                Err(SkillValidationError::MissingSkillMd(_))
            ));
        }

        #[test]
        fn rejects_a_duplicate_file() {
            let mut skill = billing_skill();
            let SkillResources::Files(files) = &mut skill.resources else {
                unreachable!()
            };
            files.push(files[0].clone());
            assert!(matches!(
                skill.validate(),
                Err(SkillValidationError::DuplicateResource(_))
            ));
        }

        #[test]
        fn rejects_a_malformed_digest() {
            let mut skill = billing_skill();
            let SkillResources::Files(files) = &mut skill.resources else {
                unreachable!()
            };
            files[0].digest = "md5:00".into();
            assert!(matches!(
                skill.validate(),
                Err(SkillValidationError::MalformedDigest { .. })
            ));
        }
    }

    mod validate_skills {
        use super::*;

        #[test]
        fn rejects_the_same_skill_listed_twice() {
            let skills = [billing_skill(), billing_skill()];
            assert!(matches!(
                validate_skills(&skills),
                Err(SkillValidationError::DuplicateSkill(_))
            ));
        }
    }

    mod get_skill_result {
        use super::*;

        #[test]
        fn new_marks_the_result_complete() {
            let value = serde_json::to_value(GetSkillResult::new(billing_skill())).unwrap();
            assert_eq!(value["resultType"], json!("complete"));
        }

        #[test]
        fn deserializes_without_cache_hints() {
            let wire = json!({ "skill": serde_json::to_value(billing_skill()).unwrap() });
            let result: GetSkillResult = serde_json::from_value(wire).unwrap();
            assert_eq!((result.ttl_ms, result.cache_scope), (None, None));
        }
    }

    mod resource_is_directory {
        use super::*;

        #[test]
        fn is_true_for_the_directory_mime_type() {
            let dir = Resource::new("skill://acme/billing", "billing")
                .with_mime_type(DIRECTORY_MIME_TYPE);
            assert!(dir.is_directory());
        }

        #[test]
        fn is_false_without_a_mime_type() {
            assert!(!Resource::new("skill://acme/billing/SKILL.md", "SKILL.md").is_directory());
        }
    }

    mod skills_capability {
        use super::*;

        #[test]
        fn serializes_to_an_empty_object_by_default() {
            assert_eq!(
                serde_json::to_value(SkillsCapability::new()).unwrap(),
                json!({})
            );
        }

        #[test]
        fn serializes_directory_read_when_set() {
            let value =
                serde_json::to_value(SkillsCapability::new().with_directory_read()).unwrap();
            assert_eq!(value, json!({ "directoryRead": true }));
        }
    }
}
