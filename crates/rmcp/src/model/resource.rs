use serde::{Deserialize, Serialize};

use super::{Annotated, Icon, Meta};

/// Represents a resource in the extension with metadata
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
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
}

pub type Resource = Annotated<RawResource>;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct RawResourceTemplate {
    pub uri_template: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

pub type ResourceTemplate = Annotated<RawResourceTemplate>;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
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
            mime_type: Some("text".into()),
            text: text.into(),
            meta: None,
        }
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
        }
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
    fn test_raw_resource_new() {
        let resource = RawResource::new("file:///test.txt", "test");
        assert_eq!(resource.uri, "file:///test.txt");
        assert_eq!(resource.name, "test");
        assert_eq!(resource.title, None);
        assert_eq!(resource.description, None);
        assert_eq!(resource.mime_type, None);
        assert_eq!(resource.size, None);
    }

    #[test]
    fn test_resource_contents_text() {
        let contents = ResourceContents::text("Hello", "file:///test.txt");
        match contents {
            ResourceContents::TextResourceContents {
                text,
                uri,
                mime_type,
                ..
            } => {
                assert_eq!(text, "Hello");
                assert_eq!(uri, "file:///test.txt");
                assert_eq!(mime_type, Some("text".to_string()));
            }
            _ => panic!("Expected TextResourceContents"),
        }
    }

    #[test]
    fn test_resource_contents_blob() {
        let blob_contents = ResourceContents::BlobResourceContents {
            uri: "file:///binary.dat".to_string(),
            mime_type: Some("application/octet-stream".to_string()),
            blob: "base64data".to_string(),
            meta: None,
        };
        let json = serde_json::to_string(&blob_contents).unwrap();
        assert!(json.contains("blob"));
        assert!(json.contains("mimeType"));
    }

    #[test]
    fn test_resource_template() {
        let template = RawResourceTemplate {
            uri_template: "file:///{path}".to_string(),
            name: "template".to_string(),
            title: Some("Template".to_string()),
            description: Some("A template".to_string()),
            mime_type: Some("text/plain".to_string()),
        };
        let json = serde_json::to_string(&template).unwrap();
        assert!(json.contains("uriTemplate"));
    }

    #[test]
    fn test_resource_with_size() {
        let resource = RawResource {
            uri: "file:///large.txt".to_string(),
            name: "large".to_string(),
            title: None,
            description: None,
            mime_type: None,
            size: Some(1024),
            icons: None,
        };
        assert_eq!(resource.size, Some(1024));
    }

    #[test]
    fn test_resource_clone() {
        let resource1 = RawResource::new("file:///test.txt", "test");
        let resource2 = resource1.clone();
        assert_eq!(resource1, resource2);
    }

    #[test]
    fn test_resource_contents_with_meta() {
        let mut meta = Meta::new();
        meta.insert("key".to_string(), serde_json::json!("value"));

        let contents = ResourceContents::TextResourceContents {
            uri: "file:///test.txt".to_string(),
            mime_type: Some("text/plain".to_string()),
            text: "content".to_string(),
            meta: Some(meta),
        };

        let json = serde_json::to_value(&contents).unwrap();
        assert_eq!(json["_meta"]["key"], "value");
    }

    #[test]
    fn test_raw_resource_with_all_fields() {
        let icon = Icon {
            src: "icon.png".to_string(),
            mime_type: Some("image/png".to_string()),
            sizes: None,
        };
        let resource = RawResource {
            uri: "file:///test.txt".to_string(),
            name: "test".to_string(),
            title: Some("Test Resource".to_string()),
            description: Some("A test resource".to_string()),
            mime_type: Some("text/plain".to_string()),
            size: Some(100),
            icons: Some(vec![icon]),
        };
        assert_eq!(resource.uri, "file:///test.txt");
        assert_eq!(resource.title, Some("Test Resource".to_string()));
        assert_eq!(resource.size, Some(100));
        assert_eq!(resource.icons.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn test_raw_resource_template_serialization() {
        let template = RawResourceTemplate {
            uri_template: "file:///{path}".to_string(),
            name: "template".to_string(),
            title: None,
            description: None,
            mime_type: None,
        };
        let json = serde_json::to_string(&template).unwrap();
        assert!(json.contains("uriTemplate"));
        assert!(json.contains("template"));
    }

    #[test]
    fn test_raw_resource_template_with_all_fields() {
        let template = RawResourceTemplate {
            uri_template: "file:///{path}".to_string(),
            name: "template".to_string(),
            title: Some("Template".to_string()),
            description: Some("A template".to_string()),
            mime_type: Some("text/plain".to_string()),
        };
        assert_eq!(template.title, Some("Template".to_string()));
        assert_eq!(template.description, Some("A template".to_string()));
    }

    #[test]
    fn test_resource_contents_text_with_none_mime() {
        let contents = ResourceContents::TextResourceContents {
            uri: "file:///test.txt".to_string(),
            mime_type: None,
            text: "content".to_string(),
            meta: None,
        };
        match contents {
            ResourceContents::TextResourceContents { mime_type, .. } => {
                assert_eq!(mime_type, None);
            }
            _ => panic!("Expected TextResourceContents"),
        }
    }

    #[test]
    fn test_resource_contents_blob_with_meta() {
        let mut meta = Meta::new();
        meta.insert("key".to_string(), serde_json::json!("value"));

        let contents = ResourceContents::BlobResourceContents {
            uri: "file:///binary.dat".to_string(),
            mime_type: Some("application/octet-stream".to_string()),
            blob: "blobdata".to_string(),
            meta: Some(meta),
        };

        let json = serde_json::to_value(&contents).unwrap();
        assert_eq!(json["_meta"]["key"], "value");
    }

    #[test]
    fn test_resource_with_different_uris() {
        let file_resource = RawResource::new("file:///path/to/file.txt", "file");
        let http_resource = RawResource::new("http://example.com/resource", "http");
        let custom_resource = RawResource::new("custom://resource/id", "custom");

        assert_eq!(file_resource.uri, "file:///path/to/file.txt");
        assert_eq!(http_resource.uri, "http://example.com/resource");
        assert_eq!(custom_resource.uri, "custom://resource/id");
    }

    #[test]
    fn test_resource_size_affects_equality() {
        let resource1 = RawResource {
            uri: "file:///test.txt".to_string(),
            name: "test".to_string(),
            title: None,
            description: None,
            mime_type: None,
            size: Some(100),
            icons: None,
        };

        let resource2 = RawResource {
            uri: "file:///test.txt".to_string(),
            name: "test".to_string(),
            title: None,
            description: None,
            mime_type: None,
            size: Some(200),
            icons: None,
        };

        assert_ne!(resource1, resource2);
    }

    #[test]
    fn test_resource_contents_text_sets_mime_type() {
        let contents = ResourceContents::text("content", "file:///test.txt");
        match contents {
            ResourceContents::TextResourceContents { mime_type, .. } => {
                assert_eq!(mime_type, Some("text".to_string()));
            }
            _ => panic!("Expected TextResourceContents"),
        }
    }

    #[test]
    fn test_resource_template_with_variable_substitution_pattern() {
        let template = RawResourceTemplate {
            uri_template: "file:///{user}/documents/{filename}".to_string(),
            name: "user_document".to_string(),
            title: Some("User Document".to_string()),
            description: Some("Access user documents by name".to_string()),
            mime_type: None,
        };

        assert!(template.uri_template.contains("{user}"));
        assert!(template.uri_template.contains("{filename}"));
    }

    #[test]
    fn test_resource_deserialization() {
        let json = r#"{
            "uri": "file:///test.txt",
            "name": "test",
            "mimeType": "text/plain"
        }"#;
        let resource: RawResource = serde_json::from_str(json).unwrap();
        assert_eq!(resource.uri, "file:///test.txt");
        assert_eq!(resource.name, "test");
        assert_eq!(resource.mime_type, Some("text/plain".to_string()));
    }

    #[test]
    fn test_resource_contents_deserialization_text() {
        let json = r#"{
            "uri": "file:///test.txt",
            "text": "content",
            "mimeType": "text/plain"
        }"#;
        let contents: ResourceContents = serde_json::from_str(json).unwrap();
        match contents {
            ResourceContents::TextResourceContents { text, .. } => {
                assert_eq!(text, "content");
            }
            _ => panic!("Expected TextResourceContents"),
        }
    }

    #[test]
    fn test_resource_contents_deserialization_blob() {
        let json = r#"{
            "uri": "file:///binary.dat",
            "blob": "blobdata",
            "mimeType": "application/octet-stream"
        }"#;
        let contents: ResourceContents = serde_json::from_str(json).unwrap();
        match contents {
            ResourceContents::BlobResourceContents { blob, .. } => {
                assert_eq!(blob, "blobdata");
            }
            _ => panic!("Expected BlobResourceContents"),
        }
    }
}
