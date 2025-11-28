//! Common utilities shared between tool and prompt handlers

use std::{any::TypeId, collections::HashMap, sync::Arc};

use schemars::JsonSchema;

use crate::{
    RoleServer, model::JsonObject, schemars::generate::SchemaSettings, service::RequestContext,
};

/// A shortcut for generating a JSON schema for a type.
pub fn schema_for_type<T: JsonSchema>() -> JsonObject {
    // explicitly to align json schema version to official specifications.
    // refer to https://github.com/modelcontextprotocol/modelcontextprotocol/pull/655 for details.
    let mut settings = SchemaSettings::draft2020_12();
    settings.transforms = vec![Box::new(schemars::transform::AddNullable::default())];
    let generator = settings.into_generator();
    let schema = generator.into_root_schema_for::<T>();
    let object = serde_json::to_value(schema).expect("failed to serialize schema");
    match object {
        serde_json::Value::Object(object) => object,
        _ => panic!(
            "Schema serialization produced non-object value: expected JSON object but got {:?}",
            object
        ),
    }
}

/// Call [`schema_for_type`] with a cache
pub fn cached_schema_for_type<T: JsonSchema + std::any::Any>() -> Arc<JsonObject> {
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
            let schema = schema_for_type::<T>();
            let schema = Arc::new(schema);
            cache
                .write()
                .expect("schema cache lock poisoned")
                .insert(TypeId::of::<T>(), schema.clone());
            schema
        }
    })
}

/// Generate and validate a JSON schema for outputSchema (must have root type "object").
pub fn schema_for_output<T: JsonSchema>() -> Result<JsonObject, String> {
    let schema = schema_for_type::<T>();

    match schema.get("type") {
        Some(serde_json::Value::String(t)) if t == "object" => Ok(schema),
        Some(serde_json::Value::String(t)) => Err(format!(
            "MCP specification requires tool outputSchema to have root type 'object', but found '{}'.",
            t
        )),
        None => Err(
            "Schema is missing 'type' field. MCP specification requires outputSchema to have root type 'object'.".to_string()
        ),
        Some(other) => Err(format!(
            "Schema 'type' field has unexpected format: {:?}. Expected \"object\".",
            other
        )),
    }
}

/// Call [`schema_for_output`] with a cache.
pub fn cached_schema_for_output<T: JsonSchema + std::any::Any>() -> Result<Arc<JsonObject>, String>
{
    thread_local! {
        static CACHE_FOR_OUTPUT: std::sync::RwLock<HashMap<TypeId, Result<Arc<JsonObject>, String>>> = Default::default();
    };
    CACHE_FOR_OUTPUT.with(|cache| {
        if let Some(result) = cache
            .read()
            .expect("output schema cache lock poisoned")
            .get(&TypeId::of::<T>())
        {
            result.clone()
        } else {
            let result = schema_for_output::<T>().map(Arc::new);
            cache
                .write()
                .expect("output schema cache lock poisoned")
                .insert(TypeId::of::<T>(), result.clone());
            result
        }
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
    use super::*;

    #[derive(serde::Serialize, serde::Deserialize, JsonSchema)]
    struct TestObject {
        value: i32,
    }

    #[test]
    fn test_schema_for_output_rejects_primitive() {
        let result = schema_for_output::<i32>();
        assert!(result.is_err(),);
    }

    #[test]
    fn test_schema_for_output_accepts_object() {
        let result = schema_for_output::<TestObject>();
        assert!(result.is_ok(),);
    }
}
