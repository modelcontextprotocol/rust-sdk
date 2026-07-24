use std::borrow::Cow;

use schemars::JsonSchema;
use serde::Serialize;

use crate::{
    handler::server::tool::IntoCallToolResult,
    model::{CallToolResponse, CallToolResult},
};

/// Json wrapper for structured output without the serialized text fallback
///
/// Like [`Json`](crate::Json), this wrapper serializes the value into the
/// `structured_content` field of the tool result and derives the tool's
/// output schema from `T`, but it does not mirror the value into `content`
/// as a text block, so large results are not sent twice.
///
/// Clients that do not read `structured_content` will see an empty `content`
/// array; see [`CallToolResult::structured_only`] for when that is safe.
#[expect(clippy::exhaustive_structs, reason = "intentionally exhaustive")]
pub struct StructuredOnly<T>(pub T);

// Implement JsonSchema for StructuredOnly<T> to delegate to T's schema
impl<T: JsonSchema> JsonSchema for StructuredOnly<T> {
    fn schema_name() -> Cow<'static, str> {
        T::schema_name()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        T::json_schema(generator)
    }
}

// Implementation for StructuredOnly<T> to create structured content without a text mirror
impl<T: Serialize + JsonSchema + 'static> IntoCallToolResult for StructuredOnly<T> {
    fn into_call_tool_result(self) -> Result<CallToolResponse, crate::ErrorData> {
        let value = serde_json::to_value(self.0).map_err(|e| {
            crate::ErrorData::internal_error(
                format!("Failed to serialize structured content: {}", e),
                None,
            )
        })?;

        Ok(CallToolResult::structured_only(value).into())
    }
}
