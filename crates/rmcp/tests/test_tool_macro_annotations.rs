#[cfg(test)]
mod tests {
    use rmcp::{ServerHandler, tool};

    #[derive(Debug, Clone, Default)]
    pub struct AnnotatedServer {}

    impl AnnotatedServer {
        // Tool with inline comments for documentation
        /// Direct annotation test tool
        /// This is used to test tool annotations
        #[tool(
            name = "direct-annotated-tool",
            annotations = {
                title: "Annotated Tool", 
                readOnlyHint: true
            }
        )]
        pub async fn direct_annotated_tool(&self, #[tool(param)] input: String) -> String {
            format!("Direct: {}", input)
        }
    }

    impl ServerHandler for AnnotatedServer {
        async fn call_tool(
            &self,
            request: rmcp::model::CallToolRequestParam,
            context: rmcp::service::RequestContext<rmcp::RoleServer>,
        ) -> Result<rmcp::model::CallToolResult, rmcp::Error> {
            let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
            match tcc.name() {
                "direct-annotated-tool" => Self::direct_annotated_tool_tool_call(tcc).await,
                _ => Err(rmcp::Error::invalid_params("method not found", None)),
            }
        }
    }

    #[test]
    fn test_direct_tool_attributes() {
        // Get the tool definition
        let tool = AnnotatedServer::direct_annotated_tool_tool_attr();

        // Verify basic properties
        assert_eq!(tool.name, "direct-annotated-tool");

        // Verify description is extracted from doc comments
        assert!(tool.description.is_some());
        assert!(
            tool.description
                .as_ref()
                .unwrap()
                .contains("Direct annotation test tool")
        );

        let annotations = tool.annotations.unwrap();
        assert_eq!(annotations.title.as_ref().unwrap(), "Annotated Tool");
        assert_eq!(annotations.read_only_hint, Some(true));
    }
}
