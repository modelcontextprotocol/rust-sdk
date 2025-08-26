use base64::engine::{Engine, general_purpose::STANDARD as BASE64_STANDARD};
use serde::{Deserialize, Serialize};

use super::{
    AnnotateAble, Annotations, RawEmbeddedResource, RawImageContent,
    content::{EmbeddedResource, ImageContent},
    resource::ResourceContents,
};

/// A prompt that can be used to generate text from a model
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct Prompt {
    /// The name of the prompt
    pub name: String,
    /// Optional description of what the prompt does
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional arguments that can be passed to customize the prompt
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<PromptArgument>>,
}

impl Prompt {
    /// Create a new prompt with the given name, description and arguments
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
            description: description.map(Into::into),
            arguments,
        }
    }
}

/// Represents a prompt argument that can be passed to customize the prompt
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct PromptArgument {
    /// The name of the argument
    pub name: String,
    /// A description of what the argument is used for
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether this argument is required
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
}

/// Represents the role of a message sender in a prompt conversation
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum PromptMessageRole {
    User,
    Assistant,
}

/// Content types that can be included in prompt messages
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum PromptMessageContent {
    /// Plain text content
    Text { text: String },
    /// Image content with base64-encoded data
    Image {
        #[serde(flatten)]
        image: ImageContent,
    },
    /// Embedded server-side resource
    Resource { resource: EmbeddedResource },
    /// A link to a resource that can be fetched separately
    ResourceLink {
        #[serde(flatten)]
        link: super::resource::Resource,
    },
}

impl PromptMessageContent {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// Create a resource link content
    pub fn resource_link(resource: super::resource::Resource) -> Self {
        Self::ResourceLink { link: resource }
    }
}

/// A message in a prompt conversation
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct PromptMessage {
    /// The role of the message sender
    pub role: PromptMessageRole,
    /// The content of the message
    pub content: PromptMessageContent,
}

impl PromptMessage {
    /// Create a new text message with the given role and text content
    pub fn new_text<S: Into<String>>(role: PromptMessageRole, text: S) -> Self {
        Self {
            role,
            content: PromptMessageContent::Text { text: text.into() },
        }
    }
    #[cfg(feature = "base64")]
    pub fn new_image(
        role: PromptMessageRole,
        data: &[u8],
        mime_type: &str,
        annotations: Option<Annotations>,
    ) -> Self {
        let mime_type = mime_type.into();

        let base64 = BASE64_STANDARD.encode(data);

        Self {
            role,
            content: PromptMessageContent::Image {
                image: RawImageContent {
                    data: base64,
                    mime_type,
                }
                .optional_annotate(annotations),
            },
        }
    }

    /// Create a new resource message
    pub fn new_resource(
        role: PromptMessageRole,
        uri: String,
        mime_type: String,
        text: Option<String>,
        annotations: Option<Annotations>,
    ) -> Self {
        let resource_contents = ResourceContents::TextResourceContents {
            uri,
            mime_type: Some(mime_type),
            text: text.unwrap_or_default(),
        };

        Self {
            role,
            content: PromptMessageContent::Resource {
                resource: RawEmbeddedResource {
                    resource: resource_contents,
                }
                .optional_annotate(annotations),
            },
        }
    }

    /// Create a new resource link message
    pub fn new_resource_link(role: PromptMessageRole, resource: super::resource::Resource) -> Self {
        Self {
            role,
            content: PromptMessageContent::ResourceLink { link: resource },
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json;

    use super::*;

    #[test]
    fn test_prompt_message_image_serialization() {
        let image_content = RawImageContent {
            data: "base64data".to_string(),
            mime_type: "image/png".to_string(),
        };

        let json = serde_json::to_string(&image_content).unwrap();
        println!("PromptMessage ImageContent JSON: {}", json);

        // Verify it contains mimeType (camelCase) not mime_type (snake_case)
        assert!(json.contains("mimeType"));
        assert!(!json.contains("mime_type"));
    }

    #[test]
    fn test_prompt_message_resource_link_serialization() {
        use super::super::resource::RawResource;

        let resource = RawResource::new("file:///test.txt", "test.txt");
        let message =
            PromptMessage::new_resource_link(PromptMessageRole::User, resource.no_annotation());

        let json = serde_json::to_string(&message).unwrap();
        println!("PromptMessage with ResourceLink JSON: {}", json);

        // Verify it contains the correct type tag
        assert!(json.contains("\"type\":\"resource_link\""));
        assert!(json.contains("\"uri\":\"file:///test.txt\""));
        assert!(json.contains("\"name\":\"test.txt\""));
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

        let content: PromptMessageContent = serde_json::from_str(json).unwrap();

        if let PromptMessageContent::ResourceLink { link } = content {
            assert_eq!(link.uri, "file:///example.txt");
            assert_eq!(link.name, "example.txt");
            assert_eq!(link.description, Some("Example file".to_string()));
            assert_eq!(link.mime_type, Some("text/plain".to_string()));
        } else {
            panic!("Expected ResourceLink variant");
        }
    }
}
