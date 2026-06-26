//! Content types that flow between agents, tools, prompts, and LLMs.
//!
//! The core union is [`ContentBlock`] (text | image | audio | resource_link | resource),
//! matching the MCP 2025-11-25 `ContentBlock` definition. Each variant carries optional
//! [`Annotations`] and `_meta` inline.
//!
//! [`SamplingMessageContentBlock`] extends the union with `tool_use` and `tool_result`
//! variants for sampling messages (SEP-1577).

// ToolUseContent/ToolResultContent are SEP-2577-deprecated; internal references are expected.
#![expect(deprecated)]
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Annotations, Meta, resource::ResourceContents};

// ---------------------------------------------------------------------------
// Flat content structs
// ---------------------------------------------------------------------------

/// Text content block (spec `TextContent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct TextContent {
    /// The text content of the message.
    pub text: String,
    /// Optional protocol-level metadata for this content block.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
    /// Optional annotations describing how the client should use this content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Annotations>,
}

impl TextContent {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            meta: None,
            annotations: None,
        }
    }

    pub fn with_meta(mut self, meta: Meta) -> Self {
        self.meta = Some(meta);
        self
    }

    pub fn with_annotations(mut self, annotations: Annotations) -> Self {
        self.annotations = Some(annotations);
        self
    }
}

/// Image content with base64-encoded data (spec `ImageContent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct ImageContent {
    /// The base64-encoded image data.
    pub data: String,
    /// The MIME type of the image (e.g. `image/png`).
    pub mime_type: String,
    /// Optional protocol-level metadata for this content block.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
    /// Optional annotations describing how the client should use this content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Annotations>,
}

impl ImageContent {
    pub fn new(data: impl Into<String>, mime_type: impl Into<String>) -> Self {
        Self {
            data: data.into(),
            mime_type: mime_type.into(),
            meta: None,
            annotations: None,
        }
    }

    pub fn with_meta(mut self, meta: Meta) -> Self {
        self.meta = Some(meta);
        self
    }

    pub fn with_annotations(mut self, annotations: Annotations) -> Self {
        self.annotations = Some(annotations);
        self
    }
}

/// Audio content with base64-encoded data (spec `AudioContent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct AudioContent {
    /// The base64-encoded audio data.
    pub data: String,
    /// The MIME type of the audio (e.g. `audio/wav`).
    pub mime_type: String,
    /// Optional protocol-level metadata for this content block.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
    /// Optional annotations describing how the client should use this content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Annotations>,
}

impl AudioContent {
    pub fn new(data: impl Into<String>, mime_type: impl Into<String>) -> Self {
        Self {
            data: data.into(),
            mime_type: mime_type.into(),
            meta: None,
            annotations: None,
        }
    }

    pub fn with_meta(mut self, meta: Meta) -> Self {
        self.meta = Some(meta);
        self
    }

    pub fn with_annotations(mut self, annotations: Annotations) -> Self {
        self.annotations = Some(annotations);
        self
    }
}

/// Embedded resource content (spec `EmbeddedResource`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct EmbeddedResource {
    /// The embedded resource contents (text or blob).
    pub resource: ResourceContents,
    /// Optional protocol-level metadata for this content block.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
    /// Optional annotations describing how the client should use this content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Annotations>,
}

impl EmbeddedResource {
    pub fn new(resource: ResourceContents) -> Self {
        Self {
            resource,
            meta: None,
            annotations: None,
        }
    }

    pub fn get_text(&self) -> String {
        match &self.resource {
            ResourceContents::TextResourceContents { text, .. } => text.clone(),
            _ => String::new(),
        }
    }

    pub fn with_meta(mut self, meta: Meta) -> Self {
        self.meta = Some(meta);
        self
    }

    pub fn with_annotations(mut self, annotations: Annotations) -> Self {
        self.annotations = Some(annotations);
        self
    }
}

/// Tool call request from assistant (SEP-1577).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
#[deprecated(
    since = "2.0.0",
    note = "Sampling is deprecated by SEP-2577 and will be removed in a future release. See https://github.com/modelcontextprotocol/modelcontextprotocol/pull/2577"
)]
pub struct ToolUseContent {
    pub id: String,
    pub name: String,
    pub input: super::JsonObject,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

/// Tool execution result in user message (SEP-1577).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
#[deprecated(
    since = "2.0.0",
    note = "Sampling is deprecated by SEP-2577 and will be removed in a future release. See https://github.com/modelcontextprotocol/modelcontextprotocol/pull/2577"
)]
pub struct ToolResultContent {
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
    pub tool_use_id: String,
    pub content: Vec<ContentBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<super::JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

impl ToolUseContent {
    pub fn new(id: impl Into<String>, name: impl Into<String>, input: super::JsonObject) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            input,
            meta: None,
        }
    }
}

impl ToolResultContent {
    pub fn new(tool_use_id: impl Into<String>, content: Vec<ContentBlock>) -> Self {
        Self {
            meta: None,
            tool_use_id: tool_use_id.into(),
            content,
            structured_content: None,
            is_error: None,
        }
    }

    pub fn error(tool_use_id: impl Into<String>, content: Vec<ContentBlock>) -> Self {
        Self {
            meta: None,
            tool_use_id: tool_use_id.into(),
            content,
            structured_content: None,
            is_error: Some(true),
        }
    }
}

// ---------------------------------------------------------------------------
// ContentBlock — the unified content union (spec `ContentBlock`)
// ---------------------------------------------------------------------------

/// Unified content block union (spec `ContentBlock`).
///
/// `text | image | audio | resource_link | resource`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub enum ContentBlock {
    Text(TextContent),
    Image(ImageContent),
    Audio(AudioContent),
    Resource(EmbeddedResource),
    ResourceLink(super::resource::Resource),
}

impl ContentBlock {
    pub fn json<S: Serialize>(json: S) -> Result<Self, crate::ErrorData> {
        let json = serde_json::to_string(&json).map_err(|e| {
            crate::ErrorData::internal_error(
                "fail to serialize response to json",
                Some(json!(
                    {"reason": e.to_string()}
                )),
            )
        })?;
        Ok(ContentBlock::text(json))
    }

    pub fn text(text: impl Into<String>) -> Self {
        ContentBlock::Text(TextContent::new(text))
    }

    pub fn image(data: impl Into<String>, mime_type: impl Into<String>) -> Self {
        ContentBlock::Image(ImageContent::new(data, mime_type))
    }

    pub fn audio(data: impl Into<String>, mime_type: impl Into<String>) -> Self {
        ContentBlock::Audio(AudioContent::new(data, mime_type))
    }

    pub fn resource(resource: ResourceContents) -> Self {
        ContentBlock::Resource(EmbeddedResource::new(resource))
    }

    pub fn embedded_text(uri: impl Into<String>, content: impl Into<String>) -> Self {
        ContentBlock::Resource(EmbeddedResource::new(
            ResourceContents::TextResourceContents {
                uri: uri.into(),
                mime_type: Some("text".to_string()),
                text: content.into(),
                meta: None,
            },
        ))
    }

    pub fn resource_link(resource: super::resource::Resource) -> Self {
        ContentBlock::ResourceLink(resource)
    }

    pub fn as_text(&self) -> Option<&TextContent> {
        match self {
            ContentBlock::Text(text) => Some(text),
            _ => None,
        }
    }

    pub fn as_image(&self) -> Option<&ImageContent> {
        match self {
            ContentBlock::Image(image) => Some(image),
            _ => None,
        }
    }

    pub fn as_resource(&self) -> Option<&EmbeddedResource> {
        match self {
            ContentBlock::Resource(resource) => Some(resource),
            _ => None,
        }
    }

    pub fn as_resource_link(&self) -> Option<&super::resource::Resource> {
        match self {
            ContentBlock::ResourceLink(link) => Some(link),
            _ => None,
        }
    }

    pub fn as_audio(&self) -> Option<&AudioContent> {
        match self {
            ContentBlock::Audio(audio) => Some(audio),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// JsonContent (unchanged)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonContent<S: Serialize>(S);

// ---------------------------------------------------------------------------
// IntoContents
// ---------------------------------------------------------------------------

/// Types that can be converted into a list of content blocks.
pub trait IntoContents {
    fn into_contents(self) -> Vec<ContentBlock>;
}

impl IntoContents for ContentBlock {
    fn into_contents(self) -> Vec<ContentBlock> {
        vec![self]
    }
}

impl IntoContents for String {
    fn into_contents(self) -> Vec<ContentBlock> {
        vec![ContentBlock::text(self)]
    }
}

impl IntoContents for () {
    fn into_contents(self) -> Vec<ContentBlock> {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use serde_json;

    use super::*;

    #[test]
    fn test_image_content_serialization() {
        let image = ImageContent::new("base64data", "image/png");
        let json = serde_json::to_string(&image).unwrap();
        assert!(json.contains("mimeType"));
        assert!(!json.contains("mime_type"));
    }

    #[test]
    fn test_audio_content_serialization() {
        let audio = AudioContent::new("base64audiodata", "audio/wav");
        let json = serde_json::to_string(&audio).unwrap();
        assert!(json.contains("mimeType"));
        assert!(!json.contains("mime_type"));
    }

    #[test]
    fn test_audio_content_has_meta() {
        let audio = AudioContent::new("data", "audio/wav").with_meta(Meta::default());
        let json = serde_json::to_value(&audio).unwrap();
        assert!(json.get("_meta").is_some());
    }

    #[test]
    fn test_resource_link_serialization() {
        use super::super::resource::Resource;

        let resource_link = ContentBlock::ResourceLink(Resource {
            uri: "file:///test.txt".to_string(),
            name: "test.txt".to_string(),
            title: None,
            description: Some("A test file".to_string()),
            mime_type: Some("text/plain".to_string()),
            size: Some(100),
            icons: None,
            meta: None,
            annotations: None,
        });

        let json = serde_json::to_string(&resource_link).unwrap();
        assert!(json.contains("\"type\":\"resource_link\""));
        assert!(json.contains("\"uri\":\"file:///test.txt\""));
        assert!(json.contains("\"name\":\"test.txt\""));
    }

    #[test]
    fn test_resource_link_deserialization() {
        let json = r#"{
            "type": "resource_link",
            "uri": "file:///example.txt",
            "name": "example.txt",
            "description": "Example file",
            "mimeType": "text/plain"
        }"#;

        let content: ContentBlock = serde_json::from_str(json).unwrap();

        if let ContentBlock::ResourceLink(resource) = content {
            assert_eq!(resource.uri, "file:///example.txt");
            assert_eq!(resource.name, "example.txt");
            assert_eq!(resource.description, Some("Example file".to_string()));
            assert_eq!(resource.mime_type, Some("text/plain".to_string()));
        } else {
            panic!("Expected ResourceLink variant");
        }
    }

    #[test]
    fn test_content_block_text_with_annotations() {
        let block = ContentBlock::Text(
            TextContent::new("hello").with_annotations(Annotations::default().with_priority(0.8)),
        );
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "text");
        assert_eq!(json["text"], "hello");
        assert_eq!(json["annotations"]["priority"], 0.8_f32);
    }
}
