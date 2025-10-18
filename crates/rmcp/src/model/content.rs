//! Content sent around agents, extensions, and LLMs
//! The various content types can be display to humans but also understood by models
//! They include optional annotations used to help inform agent usage
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{AnnotateAble, Annotated, resource::ResourceContents};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct RawTextContent {
    pub text: String,
    /// Optional protocol-level metadata for this content block
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<super::Meta>,
}
pub type TextContent = Annotated<RawTextContent>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct RawImageContent {
    /// The base64-encoded image
    pub data: String,
    pub mime_type: String,
    /// Optional protocol-level metadata for this content block
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<super::Meta>,
}

pub type ImageContent = Annotated<RawImageContent>;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct RawEmbeddedResource {
    /// Optional protocol-level metadata for this content block
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<super::Meta>,
    pub resource: ResourceContents,
}
pub type EmbeddedResource = Annotated<RawEmbeddedResource>;

impl EmbeddedResource {
    pub fn get_text(&self) -> String {
        match &self.resource {
            ResourceContents::TextResourceContents { text, .. } => text.clone(),
            _ => String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct RawAudioContent {
    pub data: String,
    pub mime_type: String,
}

pub type AudioContent = Annotated<RawAudioContent>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum RawContent {
    Text(RawTextContent),
    Image(RawImageContent),
    Resource(RawEmbeddedResource),
    Audio(RawAudioContent),
    ResourceLink(super::resource::RawResource),
}

pub type Content = Annotated<RawContent>;

impl RawContent {
    pub fn json<S: Serialize>(json: S) -> Result<Self, crate::ErrorData> {
        let json = serde_json::to_string(&json).map_err(|e| {
            crate::ErrorData::internal_error(
                "fail to serialize response to json",
                Some(json!(
                    {"reason": e.to_string()}
                )),
            )
        })?;
        Ok(RawContent::text(json))
    }

    pub fn text<S: Into<String>>(text: S) -> Self {
        RawContent::Text(RawTextContent {
            text: text.into(),
            meta: None,
        })
    }

    pub fn image<S: Into<String>, T: Into<String>>(data: S, mime_type: T) -> Self {
        RawContent::Image(RawImageContent {
            data: data.into(),
            mime_type: mime_type.into(),
            meta: None,
        })
    }

    pub fn resource(resource: ResourceContents) -> Self {
        RawContent::Resource(RawEmbeddedResource {
            meta: None,
            resource,
        })
    }

    pub fn embedded_text<S: Into<String>, T: Into<String>>(uri: S, content: T) -> Self {
        RawContent::Resource(RawEmbeddedResource {
            meta: None,
            resource: ResourceContents::TextResourceContents {
                uri: uri.into(),
                mime_type: Some("text".to_string()),
                text: content.into(),
                meta: None,
            },
        })
    }

    /// Get the text content if this is a TextContent variant
    pub fn as_text(&self) -> Option<&RawTextContent> {
        match self {
            RawContent::Text(text) => Some(text),
            _ => None,
        }
    }

    /// Get the image content if this is an ImageContent variant
    pub fn as_image(&self) -> Option<&RawImageContent> {
        match self {
            RawContent::Image(image) => Some(image),
            _ => None,
        }
    }

    /// Get the resource content if this is an ImageContent variant
    pub fn as_resource(&self) -> Option<&RawEmbeddedResource> {
        match self {
            RawContent::Resource(resource) => Some(resource),
            _ => None,
        }
    }

    /// Get the resource link if this is a ResourceLink variant
    pub fn as_resource_link(&self) -> Option<&super::resource::RawResource> {
        match self {
            RawContent::ResourceLink(link) => Some(link),
            _ => None,
        }
    }

    /// Create a resource link content
    pub fn resource_link(resource: super::resource::RawResource) -> Self {
        RawContent::ResourceLink(resource)
    }
}

impl Content {
    pub fn text<S: Into<String>>(text: S) -> Self {
        RawContent::text(text).no_annotation()
    }

    pub fn image<S: Into<String>, T: Into<String>>(data: S, mime_type: T) -> Self {
        RawContent::image(data, mime_type).no_annotation()
    }

    pub fn resource(resource: ResourceContents) -> Self {
        RawContent::resource(resource).no_annotation()
    }

    pub fn embedded_text<S: Into<String>, T: Into<String>>(uri: S, content: T) -> Self {
        RawContent::embedded_text(uri, content).no_annotation()
    }

    pub fn json<S: Serialize>(json: S) -> Result<Self, crate::ErrorData> {
        RawContent::json(json).map(|c| c.no_annotation())
    }

    /// Create a resource link content
    pub fn resource_link(resource: super::resource::RawResource) -> Self {
        RawContent::resource_link(resource).no_annotation()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonContent<S: Serialize>(S);
/// Types that can be converted into a list of contents
pub trait IntoContents {
    fn into_contents(self) -> Vec<Content>;
}

impl IntoContents for Content {
    fn into_contents(self) -> Vec<Content> {
        vec![self]
    }
}

impl IntoContents for String {
    fn into_contents(self) -> Vec<Content> {
        vec![Content::text(self)]
    }
}

impl IntoContents for () {
    fn into_contents(self) -> Vec<Content> {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use serde_json;

    use super::*;

    #[test]
    fn test_image_content_serialization() {
        let image_content = RawImageContent {
            data: "base64data".to_string(),
            mime_type: "image/png".to_string(),
            meta: None,
        };

        let json = serde_json::to_string(&image_content).unwrap();
        println!("ImageContent JSON: {}", json);

        // Verify it contains mimeType (camelCase) not mime_type (snake_case)
        assert!(json.contains("mimeType"));
        assert!(!json.contains("mime_type"));
    }

    #[test]
    fn test_audio_content_serialization() {
        let audio_content = RawAudioContent {
            data: "base64audiodata".to_string(),
            mime_type: "audio/wav".to_string(),
        };

        let json = serde_json::to_string(&audio_content).unwrap();
        println!("AudioContent JSON: {}", json);

        // Verify it contains mimeType (camelCase) not mime_type (snake_case)
        assert!(json.contains("mimeType"));
        assert!(!json.contains("mime_type"));
    }

    #[test]
    fn test_resource_link_serialization() {
        use super::super::resource::RawResource;

        let resource_link = RawContent::ResourceLink(RawResource {
            uri: "file:///test.txt".to_string(),
            name: "test.txt".to_string(),
            title: None,
            description: Some("A test file".to_string()),
            mime_type: Some("text/plain".to_string()),
            size: Some(100),
            icons: None,
        });

        let json = serde_json::to_string(&resource_link).unwrap();
        println!("ResourceLink JSON: {}", json);

        // Verify it contains the correct type tag
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

        let content: RawContent = serde_json::from_str(json).unwrap();

        if let RawContent::ResourceLink(resource) = content {
            assert_eq!(resource.uri, "file:///example.txt");
            assert_eq!(resource.name, "example.txt");
            assert_eq!(resource.description, Some("Example file".to_string()));
            assert_eq!(resource.mime_type, Some("text/plain".to_string()));
        } else {
            panic!("Expected ResourceLink variant");
        }
    }

    #[test]
    fn test_raw_content_text() {
        let content = RawContent::text("Hello");
        match content {
            RawContent::Text(text) => assert_eq!(text.text, "Hello"),
            _ => panic!("Expected Text variant"),
        }
    }

    #[test]
    fn test_raw_content_image() {
        let content = RawContent::image("base64data", "image/png");
        match content {
            RawContent::Image(image) => {
                assert_eq!(image.data, "base64data");
                assert_eq!(image.mime_type, "image/png");
            }
            _ => panic!("Expected Image variant"),
        }
    }

    #[test]
    fn test_raw_content_json() {
        let data = json!({"key": "value"});
        let content = RawContent::json(data).unwrap();
        match content {
            RawContent::Text(text) => assert!(text.text.contains("key")),
            _ => panic!("Expected Text variant"),
        }
    }

    #[test]
    fn test_raw_content_as_text() {
        let content = RawContent::text("test");
        assert!(content.as_text().is_some());
        assert!(content.as_image().is_none());
        assert!(content.as_resource().is_none());
    }

    #[test]
    fn test_raw_content_as_image() {
        let content = RawContent::image("data", "image/png");
        assert!(content.as_image().is_some());
        assert!(content.as_text().is_none());
        assert!(content.as_resource().is_none());
    }

    #[test]
    fn test_raw_content_as_resource_link() {
        use super::super::resource::RawResource;
        let resource = RawResource::new("file:///test.txt", "test.txt");
        let content = RawContent::resource_link(resource);
        assert!(content.as_resource_link().is_some());
        assert!(content.as_text().is_none());
    }

    #[test]
    fn test_raw_content_embedded_text() {
        let content = RawContent::embedded_text("file:///test.txt", "content");
        match content {
            RawContent::Resource(embedded) => match embedded.resource {
                ResourceContents::TextResourceContents { text, .. } => {
                    assert_eq!(text, "content");
                }
                _ => panic!("Expected TextResourceContents"),
            },
            _ => panic!("Expected Resource variant"),
        }
    }

    #[test]
    fn test_content_text() {
        let content = Content::text("Hello");
        assert!(content.as_text().is_some());
    }

    #[test]
    fn test_content_image() {
        let content = Content::image("data", "image/png");
        assert!(content.as_image().is_some());
    }

    #[test]
    fn test_content_json() {
        let data = json!({"test": "value"});
        let content = Content::json(data).unwrap();
        assert!(content.as_text().is_some());
    }

    #[test]
    fn test_embedded_resource_get_text() {
        let resource = RawEmbeddedResource {
            meta: None,
            resource: ResourceContents::TextResourceContents {
                uri: "file:///test.txt".to_string(),
                mime_type: Some("text/plain".to_string()),
                text: "content".to_string(),
                meta: None,
            },
        };
        let embedded: EmbeddedResource = resource.no_annotation();
        assert_eq!(embedded.get_text(), "content");
    }

    #[test]
    fn test_embedded_resource_get_text_blob() {
        let resource = RawEmbeddedResource {
            meta: None,
            resource: ResourceContents::BlobResourceContents {
                uri: "file:///test.bin".to_string(),
                mime_type: Some("application/octet-stream".to_string()),
                blob: "blobdata".to_string(),
                meta: None,
            },
        };
        let embedded: EmbeddedResource = resource.no_annotation();
        assert_eq!(embedded.get_text(), String::new());
    }

    #[test]
    fn test_into_contents_content() {
        let content = Content::text("test");
        let contents = content.into_contents();
        assert_eq!(contents.len(), 1);
    }

    #[test]
    fn test_into_contents_string() {
        let contents = "test".to_string().into_contents();
        assert_eq!(contents.len(), 1);
        assert!(contents[0].as_text().is_some());
    }

    #[test]
    fn test_into_contents_unit() {
        let contents = ().into_contents();
        assert_eq!(contents.len(), 0);
    }

    #[test]
    fn test_raw_text_content_with_meta() {
        let meta = Some(super::super::Meta::default());
        let content = RawTextContent {
            text: "test".to_string(),
            meta,
        };
        assert!(content.meta.is_some());
    }

    #[test]
    fn test_raw_image_content_with_meta() {
        let meta = Some(super::super::Meta::default());
        let content = RawImageContent {
            data: "data".to_string(),
            mime_type: "image/png".to_string(),
            meta,
        };
        assert!(content.meta.is_some());
    }

    #[test]
    fn test_raw_content_resource() {
        let resource_contents = ResourceContents::text("test content", "file:///test.txt");
        let content = RawContent::resource(resource_contents.clone());
        match content {
            RawContent::Resource(embedded) => {
                assert_eq!(embedded.resource, resource_contents);
            }
            _ => panic!("Expected Resource variant"),
        }
    }

    #[test]
    fn test_content_resource() {
        let resource_contents = ResourceContents::text("test", "file:///test.txt");
        let content = Content::resource(resource_contents);
        assert!(content.as_resource().is_some());
    }

    #[test]
    fn test_content_embedded_text() {
        let content = Content::embedded_text("file:///test.txt", "test content");
        match content.raw {
            RawContent::Resource(embedded) => match embedded.resource {
                ResourceContents::TextResourceContents { text, .. } => {
                    assert_eq!(text, "test content");
                }
                _ => panic!("Expected TextResourceContents"),
            },
            _ => panic!("Expected Resource variant"),
        }
    }

    #[test]
    fn test_content_resource_link() {
        use super::super::resource::RawResource;
        let resource = RawResource::new("file:///test.txt", "test.txt");
        let content = Content::resource_link(resource);
        assert!(content.as_resource_link().is_some());
    }

    #[test]
    fn test_raw_audio_content_creation() {
        let audio = RawAudioContent {
            data: "audiodata".to_string(),
            mime_type: "audio/mp3".to_string(),
        };
        assert_eq!(audio.data, "audiodata");
        assert_eq!(audio.mime_type, "audio/mp3");
    }

    #[test]
    fn test_raw_content_audio_variant() {
        let audio = RawAudioContent {
            data: "audiodata".to_string(),
            mime_type: "audio/wav".to_string(),
        };
        let content = RawContent::Audio(audio.clone());
        match content {
            RawContent::Audio(a) => assert_eq!(a.data, "audiodata"),
            _ => panic!("Expected Audio variant"),
        }
    }

    #[test]
    fn test_raw_content_as_methods_return_none_for_wrong_type() {
        let text_content = RawContent::text("test");
        assert!(text_content.as_image().is_none());
        assert!(text_content.as_resource().is_none());
        assert!(text_content.as_resource_link().is_none());

        let image_content = RawContent::image("data", "image/png");
        assert!(image_content.as_text().is_none());
        assert!(image_content.as_resource().is_none());
    }

    #[test]
    fn test_embedded_resource_get_text_returns_empty_for_non_text() {
        let resource = RawEmbeddedResource {
            meta: None,
            resource: ResourceContents::BlobResourceContents {
                uri: "file:///test.bin".to_string(),
                mime_type: None,
                blob: "data".to_string(),
                meta: None,
            },
        };
        let embedded: EmbeddedResource = resource.no_annotation();
        assert_eq!(embedded.get_text(), "");
    }

    #[test]
    fn test_raw_content_image_with_different_mime_types() {
        let jpeg = RawContent::image("data", "image/jpeg");
        let png = RawContent::image("data", "image/png");
        let webp = RawContent::image("data", "image/webp");

        match jpeg {
            RawContent::Image(img) => assert_eq!(img.mime_type, "image/jpeg"),
            _ => panic!("Expected Image variant"),
        }
        match png {
            RawContent::Image(img) => assert_eq!(img.mime_type, "image/png"),
            _ => panic!("Expected Image variant"),
        }
        match webp {
            RawContent::Image(img) => assert_eq!(img.mime_type, "image/webp"),
            _ => panic!("Expected Image variant"),
        }
    }

    #[test]
    fn test_json_content_json_array() {
        let data = json!([1, 2, 3]);
        let result = RawContent::json(data);
        assert!(result.is_ok());
        match result.unwrap() {
            RawContent::Text(text) => assert!(text.text.contains("1")),
            _ => panic!("Expected Text variant"),
        }
    }

    #[test]
    fn test_raw_content_text_with_meta_preserved() {
        let mut meta = super::super::Meta::default();
        meta.insert("key".to_string(), json!("value"));

        let content = RawTextContent {
            text: "test".to_string(),
            meta: Some(meta.clone()),
        };
        assert_eq!(
            content.meta.as_ref().unwrap().get("key"),
            Some(&json!("value"))
        );
    }

    #[test]
    fn test_raw_content_audio_variant_not_confused_with_image() {
        let audio = RawContent::Audio(RawAudioContent {
            data: "audiodata".to_string(),
            mime_type: "audio/mp3".to_string(),
        });

        assert!(matches!(audio, RawContent::Audio(_)));
        assert!(!matches!(audio, RawContent::Image(_)));
        assert!(audio.as_image().is_none());
    }

    #[test]
    fn test_raw_content_json_nested_object() {
        let data = json!({"outer": {"inner": "value"}});
        let content = RawContent::json(data).unwrap();
        match content {
            RawContent::Text(text) => {
                assert!(text.text.contains("outer"));
                assert!(text.text.contains("inner"));
            }
            _ => panic!("Expected Text variant"),
        }
    }
}
