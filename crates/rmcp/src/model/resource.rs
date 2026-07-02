use serde::{Deserialize, Serialize};

use super::{Annotations, Icon, Meta};

/// A known resource that the server is capable of reading (spec `Resource`).
///
/// Also used as the inner type of `ContentBlock::ResourceLink` (spec `ResourceLink extends Resource`).
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Resource {
    /// The URI of this resource (e.g. `file:///path/to/file`).
    pub uri: String,
    /// The programmatic name of the resource.
    pub name: String,
    /// Optional human-readable display title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Optional description of what this resource represents.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The MIME type of this resource, if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// The size of the raw resource content in bytes (before base64/tokenization), if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Optional set of icons the client may display for this resource.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icons: Option<Vec<Icon>>,
    /// Optional protocol-level metadata for this resource.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
    /// Optional annotations describing how the client should use this resource.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Annotations>,
}

impl Resource {
    pub fn new(uri: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            name: name.into(),
            title: None,
            description: None,
            mime_type: None,
            size: None,
            icons: None,
            meta: None,
            annotations: None,
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_mime_type(mut self, mime_type: impl Into<String>) -> Self {
        self.mime_type = Some(mime_type.into());
        self
    }

    pub fn with_size(mut self, size: u64) -> Self {
        self.size = Some(size);
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

    pub fn with_annotations(mut self, annotations: Annotations) -> Self {
        self.annotations = Some(annotations);
        self
    }
}

/// A template description for resources available on the server (spec `ResourceTemplate`).
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct ResourceTemplate {
    /// An RFC 6570 URI template for constructing resource URIs.
    pub uri_template: String,
    /// The programmatic name of the resource template.
    pub name: String,
    /// Optional human-readable display title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Optional description of what this template is for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The MIME type for resources matching this template, if uniform.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Optional set of icons the client may display for this template.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icons: Option<Vec<Icon>>,
    /// Optional protocol-level metadata for this resource template.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
    /// Optional annotations describing how the client should use this template.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Annotations>,
}

impl ResourceTemplate {
    pub fn new(uri_template: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            uri_template: uri_template.into(),
            name: name.into(),
            title: None,
            description: None,
            mime_type: None,
            icons: None,
            meta: None,
            annotations: None,
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_mime_type(mut self, mime_type: impl Into<String>) -> Self {
        self.mime_type = Some(mime_type.into());
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

    pub fn with_annotations(mut self, annotations: Annotations) -> Self {
        self.annotations = Some(annotations);
        self
    }
}

/// The contents of a specific resource or sub-resource.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub enum ResourceContents {
    #[serde(rename_all = "camelCase")]
    TextResourceContents {
        uri: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        text: String,
        #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<Meta>,
    },
    #[serde(rename_all = "camelCase")]
    BlobResourceContents {
        uri: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        blob: String,
        #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<Meta>,
    },
}

impl ResourceContents {
    pub fn text(text: impl Into<String>, uri: impl Into<String>) -> Self {
        Self::TextResourceContents {
            uri: uri.into(),
            mime_type: Some("text/plain".into()),
            text: text.into(),
            meta: None,
        }
    }

    pub fn blob(blob: impl Into<String>, uri: impl Into<String>) -> Self {
        Self::BlobResourceContents {
            uri: uri.into(),
            mime_type: None,
            blob: blob.into(),
            meta: None,
        }
    }

    pub fn with_mime_type(mut self, mime_type: impl Into<String>) -> Self {
        match &mut self {
            Self::TextResourceContents { mime_type: mt, .. } => *mt = Some(mime_type.into()),
            Self::BlobResourceContents { mime_type: mt, .. } => *mt = Some(mime_type.into()),
        }
        self
    }

    pub fn with_meta(mut self, meta: Meta) -> Self {
        match &mut self {
            Self::TextResourceContents { meta: m, .. } => *m = Some(meta),
            Self::BlobResourceContents { meta: m, .. } => *m = Some(meta),
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use serde_json;

    use super::*;
    use crate::model::IconTheme;

    #[test]
    fn test_resource_serialization() {
        let resource = Resource::new("file:///test.txt", "test")
            .with_description("Test resource")
            .with_mime_type("text/plain")
            .with_size(100);

        let json = serde_json::to_string(&resource).unwrap();
        assert!(json.contains("mimeType"));
        assert!(!json.contains("mime_type"));
    }

    #[test]
    fn test_resource_contents_serialization() {
        let text_contents = ResourceContents::TextResourceContents {
            uri: "file:///test.txt".to_string(),
            mime_type: Some("text/plain".to_string()),
            text: "Hello world".to_string(),
            meta: None,
        };

        let json = serde_json::to_string(&text_contents).unwrap();
        assert!(json.contains("mimeType"));
        assert!(!json.contains("mime_type"));
    }

    #[test]
    fn test_resource_template_with_icons() {
        let resource_template = ResourceTemplate::new("file:///{path}", "template")
            .with_title("Test Template")
            .with_description("A test resource template")
            .with_mime_type("text/plain")
            .with_icons(vec![Icon {
                src: "https://example.com/icon.png".to_string(),
                mime_type: Some("image/png".to_string()),
                sizes: Some(vec!["48x48".to_string()]),
                theme: Some(IconTheme::Light),
            }]);

        let json = serde_json::to_value(&resource_template).unwrap();
        assert!(json["icons"].is_array());
        assert_eq!(json["icons"][0]["src"], "https://example.com/icon.png");
        assert_eq!(json["icons"][0]["sizes"][0], "48x48");
        assert_eq!(json["icons"][0]["theme"], "light");
    }

    #[test]
    fn test_resource_template_without_icons() {
        let resource_template = ResourceTemplate::new("file:///{path}", "template");
        let json = serde_json::to_value(&resource_template).unwrap();
        assert!(json.get("icons").is_none());
    }

    #[test]
    fn test_resource_size_u64() {
        let resource = Resource::new("file:///big", "big").with_size(5_000_000_000);
        let json = serde_json::to_value(&resource).unwrap();
        assert_eq!(json["size"], 5_000_000_000_u64);
    }

    #[test]
    fn test_resource_with_annotations() {
        let resource = Resource::new("file:///test.txt", "test")
            .with_annotations(Annotations::default().with_priority(0.9));
        let json = serde_json::to_value(&resource).unwrap();
        assert_eq!(json["annotations"]["priority"], 0.9_f32);
    }

    #[test]
    fn test_resource_template_with_meta() {
        let resource_template =
            ResourceTemplate::new("file:///{path}", "template").with_meta(Meta::default());
        let json = serde_json::to_value(&resource_template).unwrap();
        assert!(json.get("_meta").is_some());
    }
}
