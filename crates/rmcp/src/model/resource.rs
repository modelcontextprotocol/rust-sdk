use serde::{Deserialize, Serialize};

use super::{Annotated, Icon, Meta};

/// Represents a resource in the extension with metadata
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[allow(clippy::exhaustive_structs)]
pub struct RawResource {
    /// URI representing the resource location (e.g., "file:///path/to/file" or "str:///content")
    pub uri: String,
    /// Name of the resource
    pub name: String,
    /// Human-readable title of the resource
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Optional description of the resource
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// MIME type of the resource content ("text" or "blob")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,

    /// The size of the raw resource content, in bytes (i.e., before base64 encoding or any tokenization), if known.
    ///
    /// This can be used by Hosts to display file sizes and estimate context window us
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u32>,
    /// Optional list of icons for the resource
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icons: Option<Vec<Icon>>,
    /// Optional additional metadata for this resource
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

pub type Resource = Annotated<RawResource>;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[allow(clippy::exhaustive_structs)]
pub struct RawResourceTemplate {
    pub uri_template: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Optional list of icons for the resource template
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icons: Option<Vec<Icon>>,
}

pub type ResourceTemplate = Annotated<RawResourceTemplate>;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[allow(clippy::exhaustive_enums)]
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
    /// Create text resource contents.
    pub fn text(text: impl Into<String>, uri: impl Into<String>) -> Self {
        Self::TextResourceContents {
            uri: uri.into(),
            mime_type: Some("text".into()),
            text: text.into(),
            meta: None,
        }
    }

    /// Create blob resource contents.
    pub fn blob(blob: impl Into<String>, uri: impl Into<String>) -> Self {
        Self::BlobResourceContents {
            uri: uri.into(),
            mime_type: None,
            blob: blob.into(),
            meta: None,
        }
    }

    /// Set the MIME type on this resource contents.
    pub fn with_mime_type(mut self, mime_type: impl Into<String>) -> Self {
        match &mut self {
            Self::TextResourceContents { mime_type: mt, .. } => *mt = Some(mime_type.into()),
            Self::BlobResourceContents { mime_type: mt, .. } => *mt = Some(mime_type.into()),
        }
        self
    }

    /// Set the metadata on this resource contents.
    pub fn with_meta(mut self, meta: Meta) -> Self {
        match &mut self {
            Self::TextResourceContents { meta: m, .. } => *m = Some(meta),
            Self::BlobResourceContents { meta: m, .. } => *m = Some(meta),
        }
        self
    }
}

impl RawResource {
    /// Creates a new Resource from a URI with explicit mime type
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
        }
    }

    /// Set the human-readable title.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the MIME type.
    pub fn with_mime_type(mut self, mime_type: impl Into<String>) -> Self {
        self.mime_type = Some(mime_type.into());
        self
    }

    /// Set the size in bytes.
    pub fn with_size(mut self, size: u32) -> Self {
        self.size = Some(size);
        self
    }

    /// Set the icons.
    pub fn with_icons(mut self, icons: Vec<Icon>) -> Self {
        self.icons = Some(icons);
        self
    }

    /// Set the metadata.
    pub fn with_meta(mut self, meta: Meta) -> Self {
        self.meta = Some(meta);
        self
    }
}

impl RawResourceTemplate {
    /// Creates a new RawResourceTemplate with a URI template and name.
    pub fn new(uri_template: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            uri_template: uri_template.into(),
            name: name.into(),
            title: None,
            description: None,
            mime_type: None,
            icons: None,
        }
    }

    /// Set the human-readable title.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the MIME type.
    pub fn with_mime_type(mut self, mime_type: impl Into<String>) -> Self {
        self.mime_type = Some(mime_type.into());
        self
    }

    /// Set the icons.
    pub fn with_icons(mut self, icons: Vec<Icon>) -> Self {
        self.icons = Some(icons);
        self
    }
}

#[cfg(test)]
mod tests {
    use serde_json;

    use super::*;

    #[test]
    fn test_resource_serialization() {
        let resource = RawResource {
            uri: "file:///test.txt".to_string(),
            title: None,
            name: "test".to_string(),
            description: Some("Test resource".to_string()),
            mime_type: Some("text/plain".to_string()),
            size: Some(100),
            icons: None,
            meta: None,
        };

        let json = serde_json::to_string(&resource).unwrap();
        println!("Serialized JSON: {}", json);

        // Verify it contains mimeType (camelCase) not mime_type (snake_case)
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
        println!("ResourceContents JSON: {}", json);

        // Verify it contains mimeType (camelCase) not mime_type (snake_case)
        assert!(json.contains("mimeType"));
        assert!(!json.contains("mime_type"));
    }

    #[test]
    fn test_resource_template_with_icons() {
        let resource_template = RawResourceTemplate {
            uri_template: "file:///{path}".to_string(),
            name: "template".to_string(),
            title: Some("Test Template".to_string()),
            description: Some("A test resource template".to_string()),
            mime_type: Some("text/plain".to_string()),
            icons: Some(vec![Icon {
                src: "https://example.com/icon.png".to_string(),
                mime_type: Some("image/png".to_string()),
                sizes: Some(vec!["48x48".to_string()]),
            }]),
        };

        let json = serde_json::to_value(&resource_template).unwrap();
        assert!(json["icons"].is_array());
        assert_eq!(json["icons"][0]["src"], "https://example.com/icon.png");
        assert_eq!(json["icons"][0]["sizes"][0], "48x48");
    }

    #[test]
    fn test_resource_template_without_icons() {
        let resource_template = RawResourceTemplate {
            uri_template: "file:///{path}".to_string(),
            name: "template".to_string(),
            title: None,
            description: None,
            mime_type: None,
            icons: None,
        };

        let json = serde_json::to_value(&resource_template).unwrap();
        assert!(json.get("icons").is_none());
    }
}
