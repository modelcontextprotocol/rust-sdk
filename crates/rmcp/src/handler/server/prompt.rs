//! Prompt handling infrastructure for MCP servers
//!
//! This module provides the core types and traits for implementing prompt handlers
//! in MCP servers. Prompts allow servers to provide reusable templates for LLM
//! interactions with customizable arguments.

use std::{any::TypeId, collections::HashMap, future::Future, marker::PhantomData, pin::Pin};

use futures::future::BoxFuture;
use schemars::{JsonSchema, schema_for};
use serde::de::DeserializeOwned;

use crate::{
    RoleServer,
    model::{GetPromptResult, PromptArgument, PromptMessage},
    service::RequestContext,
};

/// Context for prompt retrieval operations
pub struct PromptContext<'a, S> {
    pub server: &'a S,
    pub name: String,
    pub arguments: Option<serde_json::Map<String, serde_json::Value>>,
    pub context: RequestContext<RoleServer>,
}

impl<'a, S> PromptContext<'a, S> {
    pub fn new(
        server: &'a S,
        name: String,
        arguments: Option<serde_json::Map<String, serde_json::Value>>,
        context: RequestContext<RoleServer>,
    ) -> Self {
        Self {
            server,
            name,
            arguments,
            context,
        }
    }

    /// Invoke a prompt handler with parsed arguments
    pub async fn invoke<H, A>(self, handler: H) -> Result<GetPromptResult, crate::Error>
    where
        H: GetPromptHandler<S, A>,
        S: 'a,
    {
        handler.handle(self).await
    }
}

/// Trait for handling prompt retrieval
pub trait GetPromptHandler<S, A> {
    fn handle<'a>(
        self,
        context: PromptContext<'a, S>,
    ) -> BoxFuture<'a, Result<GetPromptResult, crate::Error>>
    where
        S: 'a;
}

/// Type alias for dynamic prompt handlers
pub type DynGetPromptHandler<S> = dyn for<'a> Fn(PromptContext<'a, S>) -> BoxFuture<'a, Result<GetPromptResult, crate::Error>>
    + Send
    + Sync;

/// Adapter type for async methods that return Vec<PromptMessage>
pub struct AsyncMethodAdapter<T>(PhantomData<T>);

/// Adapter type for async methods with arguments that return Vec<PromptMessage>  
pub struct AsyncMethodWithArgsAdapter<T>(PhantomData<T>);

/// Wrapper for parsing prompt arguments
pub struct Arguments<T>(pub T);

/// Type alias for prompt arguments - matches tool's Parameters<T> pattern
pub type PromptArguments<T> = Arguments<T>;

impl<T> Arguments<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T: JsonSchema> JsonSchema for Arguments<T> {
    fn schema_name() -> String {
        T::schema_name()
    }

    fn json_schema(generator: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
        T::json_schema(generator)
    }
}

/// Convert a JSON schema into prompt arguments
pub fn arguments_from_schema<T: JsonSchema>() -> Option<Vec<PromptArgument>> {
    let schema = schema_for!(T);
    let schema_value = serde_json::to_value(schema).ok()?;

    // Extract properties from the schema
    let properties = schema_value.get("properties")?.as_object()?;

    let required = schema_value
        .get("required")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .collect::<std::collections::HashSet<_>>()
        })
        .unwrap_or_default();

    let mut arguments = Vec::new();
    for (name, prop_schema) in properties {
        let description = prop_schema
            .get("description")
            .and_then(|d| d.as_str())
            .map(String::from);

        arguments.push(PromptArgument {
            name: name.clone(),
            description,
            required: Some(required.contains(name.as_str())),
        });
    }

    if arguments.is_empty() {
        None
    } else {
        Some(arguments)
    }
}

/// Call [`arguments_from_schema`] with a cache
pub fn cached_arguments_from_schema<T: JsonSchema + std::any::Any>() -> Option<Vec<PromptArgument>>
{
    thread_local! {
        static CACHE_FOR_TYPE: std::sync::RwLock<HashMap<TypeId, Option<Vec<PromptArgument>>>> = Default::default();
    };
    CACHE_FOR_TYPE.with(|cache| {
        // Try to read from cache first
        if let Ok(cache_read) = cache.read() {
            if let Some(x) = cache_read.get(&TypeId::of::<T>()) {
                return x.clone();
            }
        }

        // Compute the value
        let args = arguments_from_schema::<T>();

        // Try to update cache, but don't fail if we can't
        if let Ok(mut cache_write) = cache.write() {
            cache_write.insert(TypeId::of::<T>(), args.clone());
        }

        args
    })
}

// Implement GetPromptHandler for async functions returning GetPromptResult
impl<S, F, Fut> GetPromptHandler<S, ()> for F
where
    S: Sync,
    F: FnOnce(&S, RequestContext<RoleServer>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<GetPromptResult, crate::Error>> + Send + 'static,
{
    fn handle<'a>(
        self,
        context: PromptContext<'a, S>,
    ) -> BoxFuture<'a, Result<GetPromptResult, crate::Error>>
    where
        S: 'a,
    {
        Box::pin(async move { (self)(context.server, context.context).await })
    }
}

// Implement GetPromptHandler for async functions with parsed arguments
impl<S, F, Fut, T> GetPromptHandler<S, Arguments<T>> for F
where
    S: Sync,
    F: FnOnce(&S, Arguments<T>, RequestContext<RoleServer>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<GetPromptResult, crate::Error>> + Send + 'static,
    T: DeserializeOwned + 'static,
{
    fn handle<'a>(
        self,
        context: PromptContext<'a, S>,
    ) -> BoxFuture<'a, Result<GetPromptResult, crate::Error>>
    where
        S: 'a,
    {
        Box::pin(async move {
            // Parse arguments if provided
            let args = if let Some(args_map) = context.arguments {
                let args_value = serde_json::Value::Object(args_map);
                serde_json::from_value::<T>(args_value).map_err(|e| {
                    crate::Error::invalid_params(format!("Failed to parse arguments: {}", e), None)
                })?
            } else {
                // Try to deserialize from empty object for optional fields
                serde_json::from_value::<T>(serde_json::json!({})).map_err(|e| {
                    crate::Error::invalid_params(format!("Missing required arguments: {}", e), None)
                })?
            };

            (self)(context.server, Arguments(args), context.context).await
        })
    }
}

// Implement GetPromptHandler for async methods that return Pin<Box<dyn Future>>
impl<S, F> GetPromptHandler<S, AsyncMethodAdapter<Vec<PromptMessage>>> for F
where
    S: Sync + 'static,
    F: for<'a> FnOnce(
            &'a S,
            RequestContext<RoleServer>,
        ) -> Pin<
            Box<dyn Future<Output = Result<Vec<PromptMessage>, crate::Error>> + Send + 'a>,
        > + Send
        + 'static,
{
    fn handle<'a>(
        self,
        context: PromptContext<'a, S>,
    ) -> BoxFuture<'a, Result<GetPromptResult, crate::Error>>
    where
        S: 'a,
    {
        Box::pin(async move {
            let messages = (self)(context.server, context.context).await?;
            Ok(GetPromptResult {
                description: None,
                messages,
            })
        })
    }
}

// Implement GetPromptHandler for async methods with arguments that return Pin<Box<dyn Future>>
impl<S, F, T> GetPromptHandler<S, AsyncMethodWithArgsAdapter<(Arguments<T>, Vec<PromptMessage>)>>
    for F
where
    S: Sync + 'static,
    T: DeserializeOwned + 'static,
    F: for<'a> FnOnce(
            &'a S,
            Arguments<T>,
            RequestContext<RoleServer>,
        ) -> Pin<
            Box<dyn Future<Output = Result<Vec<PromptMessage>, crate::Error>> + Send + 'a>,
        > + Send
        + 'static,
{
    fn handle<'a>(
        self,
        context: PromptContext<'a, S>,
    ) -> BoxFuture<'a, Result<GetPromptResult, crate::Error>>
    where
        S: 'a,
    {
        Box::pin(async move {
            // Parse arguments if provided
            let args = if let Some(args_map) = context.arguments {
                let args_value = serde_json::Value::Object(args_map);
                serde_json::from_value::<T>(args_value).map_err(|e| {
                    crate::Error::invalid_params(format!("Failed to parse arguments: {}", e), None)
                })?
            } else {
                // Try to deserialize from empty object for optional fields
                serde_json::from_value::<T>(serde_json::json!({})).map_err(|e| {
                    crate::Error::invalid_params(format!("Missing required arguments: {}", e), None)
                })?
            };

            let messages = (self)(context.server, Arguments(args), context.context).await?;
            Ok(GetPromptResult {
                description: None,
                messages,
            })
        })
    }
}

// Implement GetPromptHandler for async functions returning Vec<PromptMessage>
impl<S, F, Fut> GetPromptHandler<S, ((), Vec<PromptMessage>)> for F
where
    S: Sync,
    F: FnOnce(&S, RequestContext<RoleServer>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<Vec<PromptMessage>, crate::Error>> + Send + 'static,
{
    fn handle<'a>(
        self,
        context: PromptContext<'a, S>,
    ) -> BoxFuture<'a, Result<GetPromptResult, crate::Error>>
    where
        S: 'a,
    {
        Box::pin(async move {
            let messages = (self)(context.server, context.context).await?;
            Ok(GetPromptResult {
                description: None,
                messages,
            })
        })
    }
}

// Implement GetPromptHandler for async functions with parsed arguments returning Vec<PromptMessage>
impl<S, F, Fut, T> GetPromptHandler<S, (Arguments<T>, Vec<PromptMessage>)> for F
where
    S: Sync,
    F: FnOnce(&S, Arguments<T>, RequestContext<RoleServer>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<Vec<PromptMessage>, crate::Error>> + Send + 'static,
    T: DeserializeOwned + 'static,
{
    fn handle<'a>(
        self,
        context: PromptContext<'a, S>,
    ) -> BoxFuture<'a, Result<GetPromptResult, crate::Error>>
    where
        S: 'a,
    {
        Box::pin(async move {
            // Parse arguments if provided
            let args = if let Some(args_map) = context.arguments {
                let args_value = serde_json::Value::Object(args_map);
                serde_json::from_value::<T>(args_value).map_err(|e| {
                    crate::Error::invalid_params(format!("Failed to parse arguments: {}", e), None)
                })?
            } else {
                // Try to deserialize from empty object for optional fields
                serde_json::from_value::<T>(serde_json::json!({})).map_err(|e| {
                    crate::Error::invalid_params(format!("Missing required arguments: {}", e), None)
                })?
            };

            let messages = (self)(context.server, Arguments(args), context.context).await?;
            Ok(GetPromptResult {
                description: None,
                messages,
            })
        })
    }
}
