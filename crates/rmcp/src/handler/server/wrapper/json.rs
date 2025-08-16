use std::borrow::Cow;

use schemars::JsonSchema;

/// Json wrapper for structured output
///
/// When used with tools, this wrapper indicates that the value should be
/// serialized as structured JSON content with an associated schema.
/// The framework will place the JSON in the `structured_content` field
/// of the tool result rather than the regular `content` field.
pub struct Json<T>(pub T);

// Implement JsonSchema for Json<T> to delegate to T's schema
impl<T: JsonSchema> JsonSchema for Json<T> {
    fn schema_name() -> Cow<'static, str> {
        T::schema_name()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        T::json_schema(generator)
    }
}
