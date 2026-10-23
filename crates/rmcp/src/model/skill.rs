//! Skill types for the MCP Skills extension (SEP-2640).
//!
//! A skill is a directory of files served over the Resources primitive:
//! `skills/list` and `skills/get` return the manifest of each skill, and
//! `resources/read` serves the file bytes. The request and result wiring
//! lives in the parent module next to the other methods.

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
