use serde::{Deserialize, Serialize};

use super::{
    Annotations, ContentBlock, Icon, Meta, Role,
    content::{AudioContent, EmbeddedResource, ImageContent, TextContent},
    resource::ResourceContents,
};

/// A prompt or prompt template that the server offers (spec `Prompt`).
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Prompt {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<PromptArgument>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icons: Option<Vec<Icon>>,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

impl Prompt {
    pub fn new<N, D>(
        name: N,
        description: Option<D>,
        arguments: Option<Vec<PromptArgument>>,
    ) -> Self
    where
        N: Into<String>,
        D: Into<String>,
    {
        Prompt {
            name: name.into(),
            title: None,
            description: description.map(Into::into),
            arguments,
            icons: None,
            meta: None,
        }
    }

    pub fn from_raw(
        name: impl Into<String>,
        description: Option<impl Into<String>>,
        arguments: Option<Vec<PromptArgument>>,
    ) -> Self {
        Prompt {
            name: name.into(),
            title: None,
            description: description.map(Into::into),
            arguments,
            icons: None,
            meta: None,
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn with_icons(mut self, icons: Vec<Icon>) -> Self {
        self.icons = Some(icons);
        self
    }

    pub fn with_meta(mut self, meta: Meta) -> Self {
        self.meta = Some(meta);
        self
    }
}

/// Describes an argument that a prompt can accept (spec `PromptArgument`).
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct PromptArgument {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
}

impl PromptArgument {
    pub fn new<N: Into<String>>(name: N) -> Self {
        PromptArgument {
            name: name.into(),
            title: None,
            description: None,
            required: None,
        }
    }

    pub fn with_title<T: Into<String>>(mut self, title: T) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn with_description<D: Into<String>>(mut self, description: D) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_required(mut self, required: bool) -> Self {
        self.required = Some(required);
        self
    }
}

/// A message returned as part of a prompt (spec `PromptMessage`).
///
/// Uses the unified `ContentBlock` for its content (text | image | audio | resource_link | resource).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct PromptMessage {
    pub role: Role,
    pub content: ContentBlock,
}

impl PromptMessage {
    pub fn new(role: Role, content: ContentBlock) -> Self {
        Self { role, content }
    }

    pub fn new_text<S: Into<String>>(role: Role, text: S) -> Self {
        Self {
            role,
            content: ContentBlock::text(text),
        }
    }

    #[cfg(feature = "base64")]
    pub fn new_image(
        role: Role,
        data: &[u8],
        mime_type: &str,
        meta: Option<Meta>,
        annotations: Option<Annotations>,
    ) -> Self {
        use base64::{Engine, prelude::BASE64_STANDARD};

        let base64 = BASE64_STANDARD.encode(data);
        Self {
            role,
            content: ContentBlock::Image(ImageContent {
                data: base64,
                mime_type: mime_type.into(),
                meta,
                annotations,
            }),
        }
    }

    #[cfg(feature = "base64")]
    pub fn new_audio(
        role: Role,
        data: &[u8],
        mime_type: &str,
        meta: Option<Meta>,
        annotations: Option<Annotations>,
    ) -> Self {
        use base64::{Engine, prelude::BASE64_STANDARD};

        let base64 = BASE64_STANDARD.encode(data);
        Self {
            role,
            content: ContentBlock::Audio(AudioContent {
                data: base64,
                mime_type: mime_type.into(),
                meta,
                annotations,
            }),
        }
    }

    pub fn new_resource(
        role: Role,
        uri: String,
        mime_type: Option<String>,
        text: Option<String>,
        resource_meta: Option<Meta>,
        resource_content_meta: Option<Meta>,
        annotations: Option<Annotations>,
    ) -> Self {
        let resource_contents = match text {
            Some(t) => ResourceContents::TextResourceContents {
                uri,
                mime_type,
                text: t,
                meta: resource_content_meta,
            },
            None => ResourceContents::BlobResourceContents {
                uri,
                mime_type,
                blob: String::new(),
                meta: resource_content_meta,
            },
        };
        Self {
            role,
            content: ContentBlock::Resource(EmbeddedResource {
                meta: resource_meta,
                resource: resource_contents,
                annotations,
            }),
        }
    }

    pub fn new_text_with_meta<S: Into<String>>(role: Role, text: S, meta: Option<Meta>) -> Self {
        Self {
            role,
            content: ContentBlock::Text(TextContent {
                text: text.into(),
                meta,
                annotations: None,
            }),
        }
    }

    pub fn new_resource_link(role: Role, resource: super::resource::Resource) -> Self {
        Self {
            role,
            content: ContentBlock::ResourceLink(resource),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json;

    use super::*;

    #[test]
    fn test_prompt_message_image_serialization() {
        let image = ImageContent::new("base64data", "image/png");
        let json = serde_json::to_string(&image).unwrap();
        assert!(json.contains("mimeType"));
        assert!(!json.contains("mime_type"));
    }

    #[test]
    fn test_prompt_message_audio_serialization_and_deserialization() {
        let content = ContentBlock::Audio(AudioContent::new("YXVkaW8=", "audio/wav"));
        let value = serde_json::to_value(&content).unwrap();
        assert_eq!(value.get("type").and_then(|v| v.as_str()), Some("audio"));
        assert_eq!(value.get("data").and_then(|v| v.as_str()), Some("YXVkaW8="));
        assert_eq!(
            value.get("mimeType").and_then(|v| v.as_str()),
            Some("audio/wav"),
            "expected camelCase mimeType, got: {value:#?}"
        );

        let json = r#"{"type":"audio","data":"YXVkaW8=","mimeType":"audio/wav"}"#;
        let parsed: ContentBlock = serde_json::from_str(json).unwrap();
        assert_eq!(parsed, content);
    }

    #[test]
    #[cfg(feature = "base64")]
    fn test_prompt_message_new_audio_constructor() {
        let message = PromptMessage::new_audio(Role::User, b"hello", "audio/wav", None, None);
        let value = serde_json::to_value(&message).unwrap();
        let content = value.get("content").expect("content present");
        assert_eq!(content.get("type").and_then(|v| v.as_str()), Some("audio"));
        assert_eq!(
            content.get("mimeType").and_then(|v| v.as_str()),
            Some("audio/wav")
        );
        assert_eq!(
            content.get("data").and_then(|v| v.as_str()),
            Some("aGVsbG8=")
        );
    }

    #[test]
    fn test_prompt_message_resource_link_serialization() {
        use super::super::resource::Resource;

        let resource = Resource::new("file:///test.txt", "test.txt");
        let message = PromptMessage::new_resource_link(Role::User, resource);

        let json = serde_json::to_string(&message).unwrap();
        assert!(json.contains("\"type\":\"resource_link\""));
        assert!(json.contains("\"uri\":\"file:///test.txt\""));
        assert!(json.contains("\"name\":\"test.txt\""));
    }

    #[test]
    fn test_prompt_message_resource_serialization_is_flat() {
        let message = PromptMessage::new_resource(
            Role::User,
            "alc://packages/sc/narrative".to_string(),
            Some("text/markdown".to_string()),
            Some("# Hello".to_string()),
            None,
            None,
            None,
        );

        let value: serde_json::Value = serde_json::to_value(&message).unwrap();
        let content = value.get("content").expect("content present");
        assert_eq!(
            content.get("type").and_then(|v| v.as_str()),
            Some("resource")
        );

        let resource = content
            .get("resource")
            .expect("resource field present at content level");

        assert_eq!(
            resource.get("uri").and_then(|v| v.as_str()),
            Some("alc://packages/sc/narrative"),
            "expected flat resource.uri, got: {resource:#?}"
        );
        assert_eq!(
            resource.get("mimeType").and_then(|v| v.as_str()),
            Some("text/markdown")
        );
        assert_eq!(
            resource.get("text").and_then(|v| v.as_str()),
            Some("# Hello")
        );

        assert!(
            resource.get("resource").is_none(),
            "double-nested resource detected (regression): {resource:#?}"
        );
    }

    #[test]
    fn test_prompt_message_content_resource_link_deserialization() {
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
}
