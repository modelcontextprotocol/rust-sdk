#![cfg(feature = "tool-search")]

use rmcp::{model::Tool, tool_search::ToolSearchIndex};
use serde_json::json;

fn tool(name: &str, description: &str) -> Tool {
    Tool::new(
        name.to_owned(),
        description.to_owned(),
        json!({"type": "object", "properties": {}})
            .as_object()
            .unwrap()
            .clone(),
    )
}

#[test]
fn ranking_and_full_definitions() {
    let tools = vec![
        tool("github_create_issue", "Create a GitHub issue"),
        tool("github_list_issues", "List GitHub issues"),
        tool("weather", "Get the weather forecast"),
    ];
    let index = ToolSearchIndex::new(tools.clone());
    let hits = index.search("create a GITHUB issue", 2);
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].tool, tools[0]);
    assert!(hits[0].score > hits[1].score);
    assert!(
        hits.iter()
            .all(|hit| hit.score.is_finite() && hit.score > 0.0)
    );
    assert!(index.search("", 5).is_empty());
    assert!(index.search("___", 5).is_empty());
    assert!(index.search("unmatched", 5).is_empty());
    assert!(index.search("github", 0).is_empty());
    assert!(ToolSearchIndex::new(vec![]).search("github", 5).is_empty());
}

#[test]
fn bm25_formula_and_stable_ties() {
    let index = ToolSearchIndex::new(vec![tool("x", "x"), tool("y", "")]);
    let score = index.search("x", 1)[0].score;
    let expected = 2.0_f64.ln() * 2.0 * 2.5 / (2.0 + 1.5 * (0.25 + 0.75 * 2.0 / 1.5));
    assert!((score - expected).abs() < 1e-12);
    assert_eq!(index.search("x x", 1)[0].score, score);

    let tools = vec![tool("first", "common term"), tool("second", "common term")];
    let index = ToolSearchIndex::new(tools.clone());
    for _ in 0..10 {
        assert_eq!(index.search("term common", 2)[0].tool, tools[0]);
    }
}

#[test]
fn title_parameters_unicode_and_replacement() {
    let mut definition = tool("opaque", "");
    definition.title = Some("Résumé lookup".into());
    definition.input_schema = std::sync::Arc::new(
        json!({
            "type": "object",
            "properties": {
                "repository_name": {
                    "type": "object",
                    "description": "Project identifier",
                    "properties": {"owner": {"type": "string", "description": "Maintainer"}}
                }
            }
        })
        .as_object()
        .unwrap()
        .clone(),
    );
    let mut index = ToolSearchIndex::new(vec![definition.clone()]);
    for query in ["RÉSUMÉ", "repository", "identifier", "owner", "maintainer"] {
        assert_eq!(index.search(query, 5)[0].tool, definition);
    }
    let no_parameters = ToolSearchIndex::with_parameter_indexing(vec![definition], false);
    assert!(no_parameters.search("maintainer repository", 5).is_empty());
    index.replace_tools(vec![tool("replacement", "new catalog")]);
    assert!(index.search("résumé", 5).is_empty());
    assert_eq!(index.search("catalog", 5)[0].tool.name, "replacement");
}

#[cfg(all(feature = "server", feature = "client", not(feature = "local")))]
mod server {
    use std::sync::{Arc, RwLock};

    use rmcp::{
        ErrorData, RoleServer, ServerHandler, ServiceExt,
        model::{
            CallToolRequestParams, CallToolResponse, CallToolResult, ListPromptsResult,
            ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig,
        },
        service::RequestContext,
        tool_search::{ToolSearchConfig, ToolSearchServer},
    };

    use super::*;

    struct Catalog(Arc<RwLock<Vec<Tool>>>);

    impl ServerHandler for Catalog {
        fn get_info(&self) -> ServerConfig {
            ServerConfig::new(
                ServerCapabilities::builder()
                    .enable_tools()
                    .enable_prompts()
                    .build(),
            )
        }

        async fn list_tools(
            &self,
            request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListToolsResult, ErrorData> {
            let tools = self.0.read().unwrap();
            let offset: usize = request
                .and_then(|r| r.cursor)
                .map(|cursor| cursor.parse().unwrap())
                .unwrap_or(0);
            Ok(ListToolsResult {
                tools: tools.iter().skip(offset).take(1).cloned().collect(),
                next_cursor: (offset + 1 < tools.len()).then(|| (offset + 1).to_string()),
                ..Default::default()
            })
        }

        async fn call_tool(
            &self,
            request: CallToolRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, ErrorData> {
            if !self
                .0
                .read()
                .unwrap()
                .iter()
                .any(|tool| tool.name == request.name)
            {
                return Err(ErrorData::invalid_params("unknown tool", None));
            }
            Ok(CallToolResult::structured(json!({
                "name": request.name, "arguments": request.arguments, "requestState": request.request_state
            })).into())
        }

        async fn list_prompts(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListPromptsResult, ErrorData> {
            Err(ErrorData::invalid_params("delegated prompts", None))
        }
    }

    #[tokio::test]
    async fn discovery_dispatch_refresh_and_delegation() -> anyhow::Result<()> {
        let definitions = vec![
            tool("visible", "GitHub status"),
            tool("create_issue", "Create a GitHub issue"),
            tool("list_issues", "List GitHub issues"),
        ];
        let catalog = Arc::new(RwLock::new(definitions.clone()));
        let mut config = ToolSearchConfig::default();
        config.always_visible = vec!["visible".into()];
        config.search_tool_name = "find".into();
        config.call_tool_name = "dispatch".into();
        let wrapper = ToolSearchServer::with_config(Catalog(catalog.clone()), config)?;
        assert_eq!(wrapper.get_tool("find").unwrap().name, "find");
        let (server_io, client_io) = tokio::io::duplex(16384);
        let server_task = tokio::spawn(async move {
            let server = wrapper.serve(server_io).await.unwrap();
            server.waiting().await.unwrap();
        });
        let client = ().serve(client_io).await?;
        let listing = client.list_tools(None).await?;
        let names: Vec<_> = listing
            .tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect();
        assert_eq!(names, ["visible", "find", "dispatch"]);
        assert!(listing.next_cursor.is_none());
        let search = client
            .call_tool(
                CallToolRequestParams::new("find")
                    .with_arguments(json!({"query": "github"}).as_object().unwrap().clone()),
            )
            .await?;
        let hits: Vec<Tool> =
            serde_json::from_value(search.structured_content.unwrap()["tools"].clone())?;
        assert_eq!(hits.len(), 2);
        assert!(hits.contains(&definitions[1]));
        assert!(hits.contains(&definitions[2]));
        for name in ["create_issue", "dispatch"] {
            let arguments = if name == "dispatch" {
                json!({"name": "create_issue", "arguments": {"title": "test"}})
            } else {
                json!({"title": "test"})
            };
            let result = client
                .call_tool(
                    CallToolRequestParams::new(name)
                        .with_arguments(arguments.as_object().unwrap().clone())
                        .with_request_state("opaque-state"),
                )
                .await?;
            let value = result.structured_content.unwrap();
            assert_eq!(value["name"], "create_issue");
            assert_eq!(value["arguments"], json!({"title": "test"}));
            assert_eq!(value["requestState"], "opaque-state");
        }
        for name in ["find", "dispatch"] {
            assert!(
                client
                    .call_tool(
                        CallToolRequestParams::new("dispatch")
                            .with_arguments(json!({"name": name}).as_object().unwrap().clone()),
                    )
                    .await
                    .is_err()
            );
        }
        for arguments in [
            json!({"query": "github", "max_results": 0}),
            json!({"query": "github", "max_results": 6}),
            json!({"query": "github", "max_results": -1}),
            json!({"query": 4}),
            json!({"query": "github", "extra": true}),
        ] {
            assert!(
                client
                    .call_tool(
                        CallToolRequestParams::new("find")
                            .with_arguments(arguments.as_object().unwrap().clone()),
                    )
                    .await
                    .is_err()
            );
        }
        assert!(
            client
                .list_prompts(None)
                .await
                .unwrap_err()
                .to_string()
                .contains("delegated prompts")
        );
        *catalog.write().unwrap() = vec![tool("weather", "Forecast")];
        let result = client
            .call_tool(
                CallToolRequestParams::new("find")
                    .with_arguments(json!({"query": "github"}).as_object().unwrap().clone()),
            )
            .await?;
        assert_eq!(result.structured_content.unwrap()["tools"], json!([]));
        let result = client
            .call_tool(
                CallToolRequestParams::new("find")
                    .with_arguments(json!({"query": "forecast"}).as_object().unwrap().clone()),
            )
            .await?;
        assert_eq!(
            result.structured_content.unwrap()["tools"][0]["name"],
            "weather"
        );
        *catalog.write().unwrap() = vec![tool("find", "Collision")];
        assert!(client.list_tools(None).await.is_err());
        client.cancel().await?;
        server_task.await?;
        Ok(())
    }

    #[test]
    fn invalid_configuration() {
        for (search, call, limit) in [
            ("same", "same", 5),
            ("", "call", 5),
            ("bad name", "call", 5),
            ("search", "call", 0),
        ] {
            let mut config = ToolSearchConfig::default();
            config.search_tool_name = search.into();
            config.call_tool_name = call.into();
            config.max_results = limit;
            assert!(
                ToolSearchServer::with_config(Catalog(Arc::new(RwLock::new(vec![]))), config,)
                    .is_err()
            );
        }
    }

    #[cfg(feature = "macros")]
    #[tokio::test]
    async fn tool_router_integration() -> anyhow::Result<()> {
        #[derive(Clone)]
        struct Routed {
            tool_router: rmcp::handler::server::router::tool::ToolRouter<Self>,
        }

        #[rmcp::tool_router]
        impl Routed {
            fn new() -> Self {
                Self {
                    tool_router: Self::tool_router(),
                }
            }

            #[rmcp::tool(description = "Read the weather forecast")]
            async fn forecast(&self) -> String {
                "sunny".into()
            }
        }

        #[rmcp::tool_handler(router = self.tool_router)]
        impl ServerHandler for Routed {
            fn get_info(&self) -> ServerConfig {
                ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            }
        }

        let (server_io, client_io) = tokio::io::duplex(16384);
        let server_task = tokio::spawn(async move {
            ToolSearchServer::new(Routed::new())
                .serve(server_io)
                .await
                .unwrap()
                .waiting()
                .await
                .unwrap();
        });
        let client = ().serve(client_io).await?;
        let listing = client.list_tools(None).await?;
        assert_eq!(listing.tools.len(), 2);
        let result = client
            .call_tool(
                CallToolRequestParams::new("search_tools")
                    .with_arguments(json!({"query": "weather"}).as_object().unwrap().clone()),
            )
            .await?;
        assert_eq!(
            result.structured_content.unwrap()["tools"][0]["name"],
            "forecast"
        );
        let result = client
            .call_tool(
                CallToolRequestParams::new("call_tool")
                    .with_arguments(json!({"name": "forecast"}).as_object().unwrap().clone()),
            )
            .await?;
        assert_eq!(
            result.content,
            vec![rmcp::model::ContentBlock::text("sunny")]
        );
        client.cancel().await?;
        server_task.await?;
        Ok(())
    }
}
