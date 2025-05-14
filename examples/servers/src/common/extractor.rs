use axum::http::HeaderMap;
use rmcp::Error as McpError;
use rmcp::handler::server::tool::{FromToolCallContextPart, ToolCallContext};

#[derive(Debug)]
pub struct ReqHeaders(pub HeaderMap);

impl<'a, S> FromToolCallContextPart<'a, S> for ReqHeaders {
    fn from_tool_call_context_part(
        context: ToolCallContext<'a, S>,
    ) -> Result<(Self, ToolCallContext<'a, S>), McpError> {
        match context.request_context().extensions.get::<HeaderMap>() {
            Some(headers) => Ok((ReqHeaders(headers.clone()), context)),
            None => Err(McpError::internal_error(
                "HTTP headers not found in context.",
                None,
            )),
        }
    }
}
