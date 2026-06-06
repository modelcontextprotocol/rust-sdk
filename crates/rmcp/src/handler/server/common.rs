//! Common utilities shared between tool and prompt handlers

use std::{
    any::TypeId,
    collections::HashMap,
    sync::{Arc, LazyLock},
};

use schemars::JsonSchema;

use crate::{
    RoleServer, model::JsonObject, schemars::generate::SchemaSettings, service::RequestContext,
};

/// Generates a JSON schema for a type
pub fn schema_for_type<T: JsonSchema + std::any::Any>() -> Arc<JsonObject> {
    thread_local! {
        static CACHE_FOR_TYPE: std::sync::RwLock<HashMap<TypeId, Arc<JsonObject>>> = Default::default();
    };
    CACHE_FOR_TYPE.with(|cache| {
        if let Some(x) = cache
            .read()
            .expect("schema cache lock poisoned")
            .get(&TypeId::of::<T>())
        {
            x.clone()
        } else {
            // explicitly to align json schema version to official specifications.
            // refer to https://github.com/modelcontextprotocol/modelcontextprotocol/pull/655 for details.
            let settings = SchemaSettings::draft2020_12();
            // Note: AddNullable is intentionally NOT used here because the `nullable` keyword
            // is an OpenAPI 3.0 extension, not part of JSON Schema 2020-12. Using it would
            // cause validation failures with strict JSON Schema validators.
            let generator = settings.into_generator();
            let schema = generator.into_root_schema_for::<T>();
            let object = serde_json::to_value(schema).expect("failed to serialize schema");
            let serde_json::Value::Object(object) = object else {
                panic!(
                    "Schema serialization produced non-object value: expected JSON object but got {object:?}"
                );
            };
            let schema = Arc::new(object);
            cache
                .write()
                .expect("schema cache lock poisoned")
                .insert(TypeId::of::<T>(), schema.clone());

            schema
        }
    })
}

/// Validate that the schema root is `type: "object"` (per MCP spec) and strip top-level
/// `title`/`description` (the wrapper type name and doc, which are noise to the LLM).
fn validate_and_strip(raw: &Arc<JsonObject>, purpose: &str) -> Result<Arc<JsonObject>, String> {
    match raw.get("type") {
        Some(serde_json::Value::String(t)) if t == "object" => {
            let mut object = raw.as_ref().clone();
            object.remove("title");
            object.remove("description");
            Ok(Arc::new(object))
        }
        Some(serde_json::Value::String(t)) => Err(format!(
            "MCP specification requires tool {purpose} to have root type 'object', but found '{t}'."
        )),
        None => Err(format!(
            "Schema is missing 'type' field. MCP specification requires {purpose} to have root type 'object'."
        )),
        Some(other) => Err(format!(
            "Schema 'type' field has unexpected format: {other:?}. Expected \"object\"."
        )),
    }
}

/// Generate, validate, and strip a JSON schema for inputSchema (must have root type "object";
/// top-level "title" and "description" are removed).
pub fn schema_for_input<T: JsonSchema + std::any::Any>() -> Result<Arc<JsonObject>, String> {
    thread_local! {
        static CACHE_FOR_INPUT: std::sync::RwLock<HashMap<TypeId, Result<Arc<JsonObject>, String>>> = Default::default();
    };
    CACHE_FOR_INPUT.with(|cache| {
        if let Some(result) = cache
            .read()
            .expect("input schema cache lock poisoned")
            .get(&TypeId::of::<T>())
        {
            return result.clone();
        }
        let result = validate_and_strip(&schema_for_type::<T>(), "inputSchema");
        cache
            .write()
            .expect("input schema cache lock poisoned")
            .insert(TypeId::of::<T>(), result.clone());
        result
    })
}

/// Schema used when input is empty.
pub fn schema_for_empty_input() -> Arc<JsonObject> {
    static EMPTY: LazyLock<Arc<JsonObject>> = LazyLock::new(|| {
        let mut object = JsonObject::new();
        object.insert("type".into(), serde_json::json!("object"));
        object.insert("properties".into(), serde_json::json!({}));
        Arc::new(object)
    });
    EMPTY.clone()
}

/// Strip top-level `title` and `description` from a JSON schema for outputSchema.
/// Unlike `validate_and_strip`, this performs no validation — output schemas are not
/// restricted to `type: "object"` (per SEP-2106).
fn strip_output(raw: &Arc<JsonObject>) -> Arc<JsonObject> {
    let mut object = raw.as_ref().clone();
    object.remove("title");
    object.remove("description");
    Arc::new(object)
}

/// Generate and strip a JSON schema for outputSchema (top-level "title" and
/// "description" are removed; output schemas are not restricted to root type "object").
pub fn schema_for_output<T: JsonSchema + std::any::Any>() -> Arc<JsonObject> {
    thread_local! {
        static CACHE_FOR_OUTPUT: std::sync::RwLock<HashMap<TypeId, Arc<JsonObject>>> = Default::default();
    };

    CACHE_FOR_OUTPUT.with(|cache| {
        if let Some(schema) = cache
            .read()
            .expect("output schema cache lock poisoned")
            .get(&TypeId::of::<T>())
        {
            return schema.clone();
        }

        let schema = strip_output(&schema_for_type::<T>());

        cache
            .write()
            .expect("output schema cache lock poisoned")
            .insert(TypeId::of::<T>(), schema.clone());

        schema
    })
}

/// Trait for extracting parts from a context, unifying tool and prompt extraction
pub trait FromContextPart<C>: Sized {
    fn from_context_part(context: &mut C) -> Result<Self, crate::ErrorData>;
}

/// Common extractors that can be used by both tool and prompt handlers
impl<C> FromContextPart<C> for RequestContext<RoleServer>
where
    C: AsRequestContext,
{
    fn from_context_part(context: &mut C) -> Result<Self, crate::ErrorData> {
        Ok(context.as_request_context().clone())
    }
}

impl<C> FromContextPart<C> for tokio_util::sync::CancellationToken
where
    C: AsRequestContext,
{
    fn from_context_part(context: &mut C) -> Result<Self, crate::ErrorData> {
        Ok(context.as_request_context().ct.clone())
    }
}

impl<C> FromContextPart<C> for crate::model::Extensions
where
    C: AsRequestContext,
{
    fn from_context_part(context: &mut C) -> Result<Self, crate::ErrorData> {
        Ok(context.as_request_context().extensions.clone())
    }
}

#[expect(clippy::exhaustive_structs, reason = "intentionally exhaustive")]
pub struct Extension<T>(pub T);

impl<C, T> FromContextPart<C> for Extension<T>
where
    C: AsRequestContext,
    T: Send + Sync + 'static + Clone,
{
    fn from_context_part(context: &mut C) -> Result<Self, crate::ErrorData> {
        let extension = context
            .as_request_context()
            .extensions
            .get::<T>()
            .cloned()
            .ok_or_else(|| {
                crate::ErrorData::invalid_params(
                    format!("missing extension {}", std::any::type_name::<T>()),
                    None,
                )
            })?;
        Ok(Extension(extension))
    }
}

impl<C> FromContextPart<C> for crate::Peer<RoleServer>
where
    C: AsRequestContext,
{
    fn from_context_part(context: &mut C) -> Result<Self, crate::ErrorData> {
        Ok(context.as_request_context().peer.clone())
    }
}

impl<C> FromContextPart<C> for crate::model::Meta
where
    C: AsRequestContext,
{
    fn from_context_part(context: &mut C) -> Result<Self, crate::ErrorData> {
        let request_context = context.as_request_context_mut();
        let mut meta = crate::model::Meta::default();
        std::mem::swap(&mut meta, &mut request_context.meta);
        Ok(meta)
    }
}

#[expect(clippy::exhaustive_structs, reason = "intentionally exhaustive")]
pub struct RequestId(pub crate::model::RequestId);

impl<C> FromContextPart<C> for RequestId
where
    C: AsRequestContext,
{
    fn from_context_part(context: &mut C) -> Result<Self, crate::ErrorData> {
        Ok(RequestId(context.as_request_context().id.clone()))
    }
}

/// Trait for types that can provide access to RequestContext
pub trait AsRequestContext {
    fn as_request_context(&self) -> &RequestContext<RoleServer>;
    fn as_request_context_mut(&mut self) -> &mut RequestContext<RoleServer>;
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[derive(serde::Serialize, serde::Deserialize, JsonSchema)]
    struct TestObject {
        value: i32,
    }

    #[derive(serde::Serialize, serde::Deserialize, JsonSchema)]
    struct AnotherTestObject {
        value: i32,
    }

    #[rstest]
    #[case::primitive(schema_for_type::<i32>, "integer")]
    #[case::array(schema_for_type::<Vec<i32>>, "array")]
    #[case::struct_object(schema_for_type::<TestObject>, "object")]
    fn schema_for_type_sets_expected_root_type(
        #[case] schema_fn: fn() -> Arc<JsonObject>,
        #[case] expected_type: &str,
    ) {
        let schema = schema_fn();

        assert_eq!(schema.get("type"), Some(&serde_json::json!(expected_type)));
    }

    #[test]
    fn schema_for_type_sets_array_item_type() {
        let schema = schema_for_type::<Vec<i32>>();
        let items = schema.get("items").and_then(|v| v.as_object()).unwrap();

        assert_eq!(items.get("type"), Some(&serde_json::json!("integer")));
    }

    #[test]
    fn schema_for_type_sets_struct_properties() {
        let schema = schema_for_type::<TestObject>();
        let properties = schema
            .get("properties")
            .and_then(|v| v.as_object())
            .unwrap();

        assert!(properties.contains_key("value"));
    }

    #[rstest]
    #[case::primitive(schema_for_type::<i32>)]
    #[case::struct_object(schema_for_type::<TestObject>)]
    fn test_schema_for_type_caches_schemas(#[case] schema_fn: fn() -> Arc<JsonObject>) {
        let schema1 = schema_fn();
        let schema2 = schema_fn();

        assert!(Arc::ptr_eq(&schema1, &schema2));
    }

    #[test]
    fn test_schema_for_type_different_types_different_schemas() {
        let schema1 = schema_for_type::<TestObject>();
        let schema2 = schema_for_type::<AnotherTestObject>();

        assert!(!Arc::ptr_eq(&schema1, &schema2));
    }

    #[test]
    fn test_schema_for_type_arc_can_be_shared() {
        let schema = schema_for_type::<TestObject>();
        let cloned = schema.clone();

        assert!(Arc::ptr_eq(&schema, &cloned));
    }

    #[test]
    fn test_schema_for_output_accepts_primitive() {
        let schema = schema_for_output::<i32>();
        assert_eq!(schema.get("type"), Some(&serde_json::json!("integer")));
    }

    #[test]
    fn test_schema_for_output_strips_description_for_primitive() {
        let schema = schema_for_output::<i32>();
        assert!(!schema.contains_key("description"));
    }

    #[test]
    fn test_schema_for_output_accepts_composition() {
        let schema = schema_for_output::<Option<String>>();
        let schema_str = serde_json::to_string(&schema).unwrap();
        assert!(
            schema_str.contains("anyOf")
                || schema_str.contains("oneOf")
                || schema_str.contains("null"),
            "Expected composition schema for Option<String>, got: {schema_str}"
        );
    }

    #[test]
    fn test_schema_for_output_caches_result() {
        let schema1 = schema_for_output::<i32>();
        let schema2 = schema_for_output::<i32>();
        assert!(Arc::ptr_eq(&schema1, &schema2));
    }

    #[test]
    fn test_schema_for_input_rejects_array() {
        let result = schema_for_input::<Vec<i32>>();
        assert!(result.is_err());
    }

    #[test]
    fn test_schema_for_output_accepts_unit() {
        let _schema = schema_for_output::<()>();
    }

    #[test]
    fn test_schema_for_output_accepts_object() {
        let schema = schema_for_output::<TestObject>();
        assert_eq!(schema.get("type"), Some(&serde_json::json!("object")));
    }

    #[test]
    fn test_schema_for_output_strips_top_level_title() {
        let schema = schema_for_output::<TestObject>();
        assert!(!schema.contains_key("title"));
    }

    #[test]
    fn test_schema_for_output_strips_top_level_description() {
        let schema = schema_for_output::<TestObject>();
        assert!(!schema.contains_key("description"));
    }

    #[rstest]
    #[case::input(schema_for_input::<i32>)]
    fn test_schema_for_input_rejects_primitives(
        #[case] schema_fn: fn() -> Result<Arc<JsonObject>, String>,
    ) {
        let result = schema_fn();
        assert!(result.is_err());
    }

    #[rstest]
    #[case::input(schema_for_input::<TestObject>)]
    fn test_schema_for_input_accepts_objects(
        #[case] schema_fn: fn() -> Result<Arc<JsonObject>, String>,
    ) {
        let result = schema_fn();
        assert!(result.is_ok());
    }

    #[rstest]
    #[case::input_title(schema_for_input::<TestObject>, "title")]
    #[case::input_description(schema_for_input::<TestObject>, "description")]
    fn test_schema_for_input_strips_top_level_metadata(
        #[case] schema_fn: fn() -> Result<Arc<JsonObject>, String>,
        #[case] field: &str,
    ) {
        let schema = schema_fn().unwrap();
        assert!(!schema.contains_key(field));
    }
}
