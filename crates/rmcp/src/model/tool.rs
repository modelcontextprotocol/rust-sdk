use std::{borrow::Cow, sync::Arc};

use schemars::JsonSchema;
/// Tools represent a routine that a server can execute
/// Tool calls represent requests from the client to execute one
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Icon, JsonObject};

/// A tool that can be used by a model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct Tool {
    /// The name of the tool
    pub name: Cow<'static, str>,
    /// A human-readable title for the tool
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// A description of what the tool does
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<Cow<'static, str>>,
    /// A JSON Schema object defining the expected parameters for the tool
    pub input_schema: Arc<JsonObject>,
    /// An optional JSON Schema object defining the structure of the tool's output
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Arc<JsonObject>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Optional additional tool information.
    pub annotations: Option<ToolAnnotations>,
    /// Optional list of icons for the tool
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icons: Option<Vec<Icon>>,
}

/// Additional properties describing a Tool to clients.
///
/// NOTE: all properties in ToolAnnotations are **hints**.
/// They are not guaranteed to provide a faithful description of
/// tool behavior (including descriptive properties like `title`).
///
/// Clients should never make tool use decisions based on ToolAnnotations
/// received from untrusted servers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ToolAnnotations {
    /// A human-readable title for the tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// If true, the tool does not modify its environment.
    ///
    /// Default: false
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,

    /// If true, the tool may perform destructive updates to its environment.
    /// If false, the tool performs only additive updates.
    ///
    /// (This property is meaningful only when `readOnlyHint == false`)
    ///
    /// Default: true
    /// A human-readable description of the tool's purpose.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,

    /// If true, calling the tool repeatedly with the same arguments
    /// will have no additional effect on the its environment.
    ///
    /// (This property is meaningful only when `readOnlyHint == false`)
    ///
    /// Default: false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotent_hint: Option<bool>,

    /// If true, this tool may interact with an "open world" of external
    /// entities. If false, the tool's domain of interaction is closed.
    /// For example, the world of a web search tool is open, whereas that
    /// of a memory tool is not.
    ///
    /// Default: true
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_world_hint: Option<bool>,
}

impl ToolAnnotations {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_title<T>(title: T) -> Self
    where
        T: Into<String>,
    {
        ToolAnnotations {
            title: Some(title.into()),
            ..Self::default()
        }
    }
    pub fn read_only(self, read_only: bool) -> Self {
        ToolAnnotations {
            read_only_hint: Some(read_only),
            ..self
        }
    }
    pub fn destructive(self, destructive: bool) -> Self {
        ToolAnnotations {
            destructive_hint: Some(destructive),
            ..self
        }
    }
    pub fn idempotent(self, idempotent: bool) -> Self {
        ToolAnnotations {
            idempotent_hint: Some(idempotent),
            ..self
        }
    }
    pub fn open_world(self, open_world: bool) -> Self {
        ToolAnnotations {
            open_world_hint: Some(open_world),
            ..self
        }
    }

    /// If not set, defaults to true.
    pub fn is_destructive(&self) -> bool {
        self.destructive_hint.unwrap_or(true)
    }

    /// If not set, defaults to false.
    pub fn is_idempotent(&self) -> bool {
        self.idempotent_hint.unwrap_or(false)
    }
}

impl Tool {
    /// Create a new tool with the given name and description
    pub fn new<N, D, S>(name: N, description: D, input_schema: S) -> Self
    where
        N: Into<Cow<'static, str>>,
        D: Into<Cow<'static, str>>,
        S: Into<Arc<JsonObject>>,
    {
        Tool {
            name: name.into(),
            title: None,
            description: Some(description.into()),
            input_schema: input_schema.into(),
            output_schema: None,
            annotations: None,
            icons: None,
        }
    }

    pub fn annotate(self, annotations: ToolAnnotations) -> Self {
        Tool {
            annotations: Some(annotations),
            ..self
        }
    }

    /// Set the output schema using a type that implements JsonSchema
    pub fn with_output_schema<T: JsonSchema + 'static>(mut self) -> Self {
        self.output_schema = Some(crate::handler::server::tool::cached_schema_for_type::<T>());
        self
    }

    /// Set the input schema using a type that implements JsonSchema
    pub fn with_input_schema<T: JsonSchema + 'static>(mut self) -> Self {
        self.input_schema = crate::handler::server::tool::cached_schema_for_type::<T>();
        self
    }

    /// Get the schema as json value
    pub fn schema_as_json_value(&self) -> Value {
        Value::Object(self.input_schema.as_ref().clone())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn test_tool_annotations_new() {
        let annotations = ToolAnnotations::new();
        assert_eq!(annotations.title, None);
        assert_eq!(annotations.read_only_hint, None);
        assert_eq!(annotations.destructive_hint, None);
        assert_eq!(annotations.idempotent_hint, None);
        assert_eq!(annotations.open_world_hint, None);
    }

    #[test]
    fn test_tool_annotations_with_title() {
        let annotations = ToolAnnotations::with_title("Test Tool");
        assert_eq!(annotations.title, Some("Test Tool".to_string()));
    }

    #[test]
    fn test_tool_annotations_read_only() {
        let annotations = ToolAnnotations::new().read_only(true);
        assert_eq!(annotations.read_only_hint, Some(true));
    }

    #[test]
    fn test_tool_annotations_destructive() {
        let annotations = ToolAnnotations::new().destructive(false);
        assert_eq!(annotations.destructive_hint, Some(false));
    }

    #[test]
    fn test_tool_annotations_idempotent() {
        let annotations = ToolAnnotations::new().idempotent(true);
        assert_eq!(annotations.idempotent_hint, Some(true));
    }

    #[test]
    fn test_tool_annotations_open_world() {
        let annotations = ToolAnnotations::new().open_world(false);
        assert_eq!(annotations.open_world_hint, Some(false));
    }

    #[test]
    fn test_tool_annotations_is_destructive_default() {
        let annotations = ToolAnnotations::new();
        assert!(annotations.is_destructive());
    }

    #[test]
    fn test_tool_annotations_is_destructive_explicit() {
        let annotations = ToolAnnotations::new().destructive(false);
        assert!(!annotations.is_destructive());
    }

    #[test]
    fn test_tool_annotations_is_idempotent_default() {
        let annotations = ToolAnnotations::new();
        assert!(!annotations.is_idempotent());
    }

    #[test]
    fn test_tool_annotations_is_idempotent_explicit() {
        let annotations = ToolAnnotations::new().idempotent(true);
        assert!(annotations.is_idempotent());
    }

    #[test]
    fn test_tool_annotations_chaining() {
        let annotations = ToolAnnotations::with_title("Test")
            .read_only(true)
            .destructive(false)
            .idempotent(true)
            .open_world(false);

        assert_eq!(annotations.title, Some("Test".to_string()));
        assert_eq!(annotations.read_only_hint, Some(true));
        assert_eq!(annotations.destructive_hint, Some(false));
        assert_eq!(annotations.idempotent_hint, Some(true));
        assert_eq!(annotations.open_world_hint, Some(false));
    }

    #[test]
    fn test_tool_new() {
        let schema = Arc::new(json!({"type": "object"}).as_object().unwrap().clone());
        let tool = Tool::new("test_tool", "A test tool", schema);

        assert_eq!(tool.name, "test_tool");
        assert_eq!(tool.description, Some(Cow::Borrowed("A test tool")));
        assert_eq!(tool.title, None);
        assert_eq!(tool.annotations, None);
        assert_eq!(tool.icons, None);
    }

    #[test]
    fn test_tool_annotate() {
        let schema = Arc::new(json!({"type": "object"}).as_object().unwrap().clone());
        let tool = Tool::new("test_tool", "desc", schema);
        let annotations = ToolAnnotations::with_title("Test Title");
        let annotated_tool = tool.annotate(annotations);

        assert!(annotated_tool.annotations.is_some());
        assert_eq!(
            annotated_tool.annotations.unwrap().title,
            Some("Test Title".to_string())
        );
    }

    #[test]
    fn test_tool_schema_as_json_value() {
        let schema_obj = json!({"type": "object", "properties": {}})
            .as_object()
            .unwrap()
            .clone();
        let schema = Arc::new(schema_obj);
        let tool = Tool::new("test_tool", "desc", schema);

        let json_value = tool.schema_as_json_value();
        assert!(json_value.is_object());
        assert_eq!(json_value["type"], "object");
    }

    #[test]
    fn test_tool_annotations_default() {
        let annotations = ToolAnnotations::default();
        assert_eq!(annotations.title, None);
        assert_eq!(annotations.read_only_hint, None);
    }

    #[test]
    fn test_tool_with_title() {
        let schema = Arc::new(json!({"type": "object"}).as_object().unwrap().clone());
        let tool = Tool {
            name: "test_tool".into(),
            title: Some("Test Tool".to_string()),
            description: Some("desc".into()),
            input_schema: schema,
            output_schema: None,
            annotations: None,
            icons: None,
        };
        assert_eq!(tool.title, Some("Test Tool".to_string()));
    }

    #[test]
    fn test_tool_with_icons() {
        let schema = Arc::new(json!({"type": "object"}).as_object().unwrap().clone());
        let icon = Icon {
            src: "icon.png".to_string(),
            mime_type: Some("image/png".to_string()),
            sizes: None,
        };
        let tool = Tool {
            name: "test_tool".into(),
            title: None,
            description: Some("desc".into()),
            input_schema: schema,
            output_schema: None,
            annotations: None,
            icons: Some(vec![icon]),
        };
        assert_eq!(tool.icons.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn test_tool_with_output_schema() {
        let schema = Arc::new(json!({"type": "object"}).as_object().unwrap().clone());
        let output_schema = Arc::new(json!({"type": "string"}).as_object().unwrap().clone());
        let tool = Tool {
            name: "test_tool".into(),
            title: None,
            description: Some("desc".into()),
            input_schema: schema,
            output_schema: Some(output_schema.clone()),
            annotations: None,
            icons: None,
        };
        assert!(tool.output_schema.is_some());
    }

    #[test]
    fn test_tool_with_different_schemas_not_equal() {
        let schema1 = Arc::new(
            json!({"type": "object", "properties": {"a": {"type": "string"}}})
                .as_object()
                .unwrap()
                .clone(),
        );
        let schema2 = Arc::new(
            json!({"type": "object", "properties": {"b": {"type": "number"}}})
                .as_object()
                .unwrap()
                .clone(),
        );

        let tool1 = Tool::new("test_tool", "desc", schema1);
        let tool2 = Tool::new("test_tool", "desc", schema2);

        assert_ne!(tool1, tool2);
    }

    #[test]
    fn test_tool_annotations_with_all_hints() {
        let annotations = ToolAnnotations::new()
            .read_only(true)
            .destructive(false)
            .idempotent(true)
            .open_world(false);

        assert_eq!(annotations.read_only_hint, Some(true));
        assert_eq!(annotations.destructive_hint, Some(false));
        assert_eq!(annotations.idempotent_hint, Some(true));
        assert_eq!(annotations.open_world_hint, Some(false));
    }

    #[test]
    fn test_tool_annotations_destructive_defaults_to_true() {
        let annotations1 = ToolAnnotations::new();
        let annotations2 = ToolAnnotations::new().destructive(true);

        assert!(annotations1.is_destructive());
        assert!(annotations2.is_destructive());
    }

    #[test]
    fn test_tool_annotations_idempotent_defaults_to_false() {
        let annotations1 = ToolAnnotations::new();
        let annotations2 = ToolAnnotations::new().idempotent(false);

        assert!(!annotations1.is_idempotent());
        assert!(!annotations2.is_idempotent());
    }

    #[test]
    fn test_tool_annotations_contradictory_hints() {
        // A tool can be both read-only and destructive (contradictory but allowed)
        let annotations = ToolAnnotations::new().read_only(true).destructive(true);

        assert_eq!(annotations.read_only_hint, Some(true));
        assert_eq!(annotations.destructive_hint, Some(true));
    }

    #[test]
    fn test_tool_serialization() {
        let schema = Arc::new(
            json!({"type": "object", "properties": {}})
                .as_object()
                .unwrap()
                .clone(),
        );
        let tool = Tool::new("test_tool", "A test tool", schema);
        let json = serde_json::to_string(&tool).unwrap();
        assert!(json.contains("test_tool"));
        assert!(json.contains("A test tool"));
    }

    #[test]
    fn test_tool_deserialization() {
        let json = r#"{
            "name": "test_tool",
            "description": "A test tool",
            "inputSchema": {"type": "object"}
        }"#;
        let tool: Tool = serde_json::from_str(json).unwrap();
        assert_eq!(tool.name, "test_tool");
        assert_eq!(tool.description, Some(Cow::Borrowed("A test tool")));
    }

    #[test]
    fn test_tool_with_annotations_serialization() {
        let schema = Arc::new(json!({"type": "object"}).as_object().unwrap().clone());
        let tool =
            Tool::new("test_tool", "desc", schema).annotate(ToolAnnotations::new().read_only(true));
        let json = serde_json::to_value(&tool).unwrap();
        assert_eq!(json["annotations"]["readOnlyHint"], true);
    }

    #[test]
    fn test_tool_annotations_serialization() {
        let annotations = ToolAnnotations::with_title("Test")
            .read_only(true)
            .destructive(false);
        let json = serde_json::to_string(&annotations).unwrap();
        assert!(json.contains("Test"));
        assert!(json.contains("readOnlyHint"));
    }

    #[test]
    fn test_tool_name_cow_borrowed() {
        let schema = Arc::new(json!({"type": "object"}).as_object().unwrap().clone());
        let tool = Tool::new("static_name", "desc", schema);
        assert_eq!(tool.name, "static_name");
    }

    #[test]
    fn test_tool_name_cow_owned() {
        let schema = Arc::new(json!({"type": "object"}).as_object().unwrap().clone());
        let name = String::from("dynamic_name");
        let tool = Tool::new(name, "desc", schema);
        assert_eq!(tool.name, "dynamic_name");
    }

    #[test]
    fn test_tool_annotations_is_idempotent_when_destructive() {
        let annotations = ToolAnnotations::new().destructive(true).idempotent(false);
        assert!(!annotations.is_idempotent());
    }

    #[test]
    fn test_tool_schema_as_json_value_complex() {
        let schema_obj = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "age": {"type": "number"}
            },
            "required": ["name"]
        })
        .as_object()
        .unwrap()
        .clone();
        let schema = Arc::new(schema_obj);
        let tool = Tool::new("test_tool", "desc", schema);

        let json_value = tool.schema_as_json_value();
        assert_eq!(json_value["type"], "object");
        assert!(json_value["properties"]["name"].is_object());
        assert!(json_value["required"].is_array());
    }

    #[test]
    fn test_tool_annotations_different_titles_not_equal() {
        let annotations1 = ToolAnnotations::with_title("Title1");
        let annotations2 = ToolAnnotations::with_title("Title2");
        assert_ne!(annotations1, annotations2);
    }

    #[test]
    fn test_tool_without_annotations_vs_with_annotations() {
        let schema = Arc::new(json!({"type": "object"}).as_object().unwrap().clone());
        let tool_without = Tool::new("test_tool", "desc", schema.clone());
        let tool_with = Tool::new("test_tool", "desc", schema).annotate(ToolAnnotations::new());

        assert!(tool_without.annotations.is_none());
        assert!(tool_with.annotations.is_some());
        assert_ne!(tool_without, tool_with);
    }
}
