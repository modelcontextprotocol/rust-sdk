use base64::engine::{Engine, general_purpose::STANDARD as BASE64_STANDARD};
use serde::{Deserialize, Serialize};

use super::{
    AnnotateAble, Annotations, Icon, RawEmbeddedResource, RawImageContent,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Optional description of what the prompt does
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional arguments that can be passed to customize the prompt
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<PromptArgument>>,
    /// Optional list of icons for the prompt
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icons: Option<Vec<Icon>>,
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
            title: None,
            description: description.map(Into::into),
            arguments,
            icons: None,
        }
    }
}

/// Represents a prompt argument that can be passed to customize the prompt
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct PromptArgument {
    /// The name of the argument
    pub name: String,
    /// A human-readable title for the argument
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
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

    /// Create a new image message. `meta` and `annotations` are optional.
    #[cfg(feature = "base64")]
    pub fn new_image(
        role: PromptMessageRole,
        data: &[u8],
        mime_type: &str,
        meta: Option<crate::model::Meta>,
        annotations: Option<Annotations>,
    ) -> Self {
        let base64 = BASE64_STANDARD.encode(data);
        Self {
            role,
            content: PromptMessageContent::Image {
                image: RawImageContent {
                    data: base64,
                    mime_type: mime_type.into(),
                    meta,
                }
                .optional_annotate(annotations),
            },
        }
    }

    /// Create a new resource message. `resource_meta`, `resource_content_meta`, and `annotations` are optional.
    pub fn new_resource(
        role: PromptMessageRole,
        uri: String,
        mime_type: Option<String>,
        text: Option<String>,
        resource_meta: Option<crate::model::Meta>,
        resource_content_meta: Option<crate::model::Meta>,
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
            content: PromptMessageContent::Resource {
                resource: RawEmbeddedResource {
                    meta: resource_meta,
                    resource: resource_contents,
                }
                .optional_annotate(annotations),
            },
        }
    }

    /// Note: PromptMessage text content does not carry protocol-level _meta per current schema.
    /// This function exists for API symmetry but ignores the meta parameter.
    pub fn new_text_with_meta<S: Into<String>>(
        role: PromptMessageRole,
        text: S,
        _meta: Option<crate::model::Meta>,
    ) -> Self {
        Self::new_text(role, text)
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
            meta: None,
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

    #[test]
    fn test_prompt_new() {
        let prompt = Prompt::new("test_prompt", Some("A test prompt"), None);
        assert_eq!(prompt.name, "test_prompt");
        assert_eq!(prompt.description, Some("A test prompt".to_string()));
        assert_eq!(prompt.arguments, None);
        assert_eq!(prompt.title, None);
        assert_eq!(prompt.icons, None);
    }

    #[test]
    fn test_prompt_with_arguments() {
        let arg = PromptArgument {
            name: "input".to_string(),
            title: Some("Input".to_string()),
            description: Some("Input parameter".to_string()),
            required: Some(true),
        };
        let prompt = Prompt::new("test", Some("desc"), Some(vec![arg.clone()]));
        assert_eq!(prompt.arguments.as_ref().unwrap().len(), 1);
        assert_eq!(prompt.arguments.as_ref().unwrap()[0].name, "input");
    }

    #[test]
    fn test_prompt_message_new_text() {
        let message = PromptMessage::new_text(PromptMessageRole::User, "Hello");
        assert_eq!(message.role, PromptMessageRole::User);
        match message.content {
            PromptMessageContent::Text { text } => assert_eq!(text, "Hello"),
            _ => panic!("Expected Text content"),
        }
    }

    #[test]
    fn test_prompt_message_content_text() {
        let content = PromptMessageContent::text("test message");
        match content {
            PromptMessageContent::Text { text } => assert_eq!(text, "test message"),
            _ => panic!("Expected Text content"),
        }
    }

    #[test]
    fn test_prompt_message_new_text_with_meta() {
        let meta = Some(crate::model::Meta::default());
        let message =
            PromptMessage::new_text_with_meta(PromptMessageRole::Assistant, "Response", meta);
        assert_eq!(message.role, PromptMessageRole::Assistant);
        match message.content {
            PromptMessageContent::Text { text } => assert_eq!(text, "Response"),
            _ => panic!("Expected Text content"),
        }
    }

    #[test]
    #[cfg(feature = "base64")]
    fn test_prompt_message_new_image() {
        let data = vec![1, 2, 3, 4];
        let message =
            PromptMessage::new_image(PromptMessageRole::User, &data, "image/png", None, None);
        assert_eq!(message.role, PromptMessageRole::User);
        match message.content {
            PromptMessageContent::Image { image } => {
                assert_eq!(image.mime_type, "image/png");
                assert!(!image.data.is_empty());
            }
            _ => panic!("Expected Image content"),
        }
    }

    #[test]
    fn test_prompt_message_new_resource() {
        let message = PromptMessage::new_resource(
            PromptMessageRole::Assistant,
            "file:///test.txt".to_string(),
            Some("text/plain".to_string()),
            Some("content".to_string()),
            None,
            None,
            None,
        );
        assert_eq!(message.role, PromptMessageRole::Assistant);
        match message.content {
            PromptMessageContent::Resource { .. } => {}
            _ => panic!("Expected Resource content"),
        }
    }

    #[test]
    fn test_prompt_argument_optional_fields() {
        let arg = PromptArgument {
            name: "param".to_string(),
            title: None,
            description: None,
            required: None,
        };
        assert_eq!(arg.name, "param");
        assert_eq!(arg.title, None);
    }

    #[test]
    fn test_prompt_message_role_variants() {
        let user_role = PromptMessageRole::User;
        let assistant_role = PromptMessageRole::Assistant;
        assert_eq!(user_role, PromptMessageRole::User);
        assert_eq!(assistant_role, PromptMessageRole::Assistant);
    }

    #[test]
    fn test_prompt_message_content_resource_link_constructor() {
        use super::super::resource::RawResource;
        let resource = RawResource::new("file:///test.txt", "test.txt");
        let content = PromptMessageContent::resource_link(resource.no_annotation());
        match content {
            PromptMessageContent::ResourceLink { link } => {
                assert_eq!(link.uri, "file:///test.txt");
            }
            _ => panic!("Expected ResourceLink content"),
        }
    }

    #[test]
    fn test_prompt_with_title() {
        let prompt = Prompt {
            name: "test".to_string(),
            title: Some("Test Title".to_string()),
            description: None,
            arguments: None,
            icons: None,
        };
        assert_eq!(prompt.title, Some("Test Title".to_string()));
    }

    #[test]
    fn test_prompt_with_icons() {
        let icon = Icon {
            src: "icon.png".to_string(),
            mime_type: Some("image/png".to_string()),
            sizes: None,
        };
        let prompt = Prompt {
            name: "test".to_string(),
            title: None,
            description: None,
            arguments: None,
            icons: Some(vec![icon]),
        };
        assert_eq!(prompt.icons.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn test_prompt_argument_all_fields() {
        let arg = PromptArgument {
            name: "param".to_string(),
            title: Some("Parameter".to_string()),
            description: Some("A parameter".to_string()),
            required: Some(true),
        };
        assert_eq!(arg.name, "param");
        assert_eq!(arg.title, Some("Parameter".to_string()));
        assert_eq!(arg.description, Some("A parameter".to_string()));
        assert_eq!(arg.required, Some(true));
    }

    #[test]
    fn test_prompt_message_role_serialization() {
        let json_user = serde_json::to_string(&PromptMessageRole::User).unwrap();
        let json_assistant = serde_json::to_string(&PromptMessageRole::Assistant).unwrap();
        assert!(json_user.contains("user"));
        assert!(json_assistant.contains("assistant"));
    }

    #[test]
    fn test_prompt_message_new_text_with_meta_ignores_meta() {
        let meta = Some(crate::model::Meta::default());
        let message1 = PromptMessage::new_text_with_meta(PromptMessageRole::User, "test", meta);
        let message2 = PromptMessage::new_text(PromptMessageRole::User, "test");

        // Should be equivalent since meta is ignored for text content
        assert_eq!(message1, message2);
    }

    #[test]
    fn test_prompt_arguments_required_vs_optional() {
        let required_arg = PromptArgument {
            name: "required".to_string(),
            title: None,
            description: None,
            required: Some(true),
        };

        let optional_arg = PromptArgument {
            name: "optional".to_string(),
            title: None,
            description: None,
            required: Some(false),
        };

        assert_ne!(required_arg.required, optional_arg.required);
        assert!(required_arg.required.unwrap());
        assert!(!optional_arg.required.unwrap());
    }

    #[test]
    fn test_prompt_new_with_none_description() {
        let prompt = Prompt::new::<_, String>("test", None, None);
        assert_eq!(prompt.name, "test");
        assert_eq!(prompt.description, None);
    }

    #[test]
    fn test_prompt_message_new_resource_blob() {
        let message = PromptMessage::new_resource(
            PromptMessageRole::User,
            "file:///binary.dat".to_string(),
            Some("application/octet-stream".to_string()),
            None,
            None,
            None,
            None,
        );
        match message.content {
            PromptMessageContent::Resource { resource } => match &resource.resource {
                ResourceContents::BlobResourceContents { uri, .. } => {
                    assert_eq!(uri, "file:///binary.dat");
                }
                _ => panic!("Expected BlobResourceContents"),
            },
            _ => panic!("Expected Resource content"),
        }
    }

    #[test]
    fn test_prompt_message_roles_are_distinct() {
        let user_msg = PromptMessage::new_text(PromptMessageRole::User, "user text");
        let assistant_msg = PromptMessage::new_text(PromptMessageRole::Assistant, "assistant text");

        assert_ne!(user_msg.role, assistant_msg.role);
        assert!(matches!(user_msg.role, PromptMessageRole::User));
        assert!(matches!(assistant_msg.role, PromptMessageRole::Assistant));
    }

    #[test]
    fn test_prompt_argument_without_required_field() {
        let arg = PromptArgument {
            name: "param".to_string(),
            title: None,
            description: None,
            required: None,
        };
        // When required is None, the argument's requirement is unspecified
        assert!(arg.required.is_none());
    }

    #[test]
    fn test_prompt_with_multiple_arguments() {
        let args = vec![
            PromptArgument {
                name: "arg1".to_string(),
                title: None,
                description: None,
                required: Some(true),
            },
            PromptArgument {
                name: "arg2".to_string(),
                title: None,
                description: None,
                required: Some(false),
            },
        ];
        let prompt = Prompt::new("test", Some("desc"), Some(args));
        assert_eq!(prompt.arguments.as_ref().unwrap().len(), 2);
    }
}
