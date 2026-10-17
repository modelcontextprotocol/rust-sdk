use std::borrow::Cow;

use schemars::JsonSchema;
use serde::Serialize;

use crate::{
    handler::server::tool::IntoCallToolResult,
    model::{CallToolResponse, CallToolResult},
};

/// JSON wrapper that omits the optional serialized text mirror.
///
/// Like [`Json`](crate::Json), this wrapper serializes the value into the
/// `structured_content` field of the tool result and derives the tool's
/// output schema from `T`. Object-shaped values are not mirrored into
/// `content`, so large results are not sent twice.
///
/// Per SEP-2106, arrays, primitives, and `null` retain a serialized JSON
/// [`TextContent`](crate::model::TextContent) fallback for older clients.
/// See [`CallToolResult::structured_only`] for details.
#[expect(clippy::exhaustive_structs, reason = "intentionally exhaustive")]
pub struct StructuredOnly<T>(pub T);

impl<T: JsonSchema> JsonSchema for StructuredOnly<T> {
    fn schema_name() -> Cow<'static, str> {
        T::schema_name()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        T::json_schema(generator)
    }
}

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
