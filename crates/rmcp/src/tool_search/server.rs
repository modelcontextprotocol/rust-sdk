use std::{collections::HashSet, ops::Deref};

use serde::Deserialize;
use serde_json::json;
use tokio::sync::Mutex;

use super::ToolSearchIndex;
use crate::{
    ErrorData, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, JsonObject, ListToolsResult,
        PaginatedRequestParams, Tool, ToolAnnotations,
    },
    service::RequestContext,
};

/// Discovery settings. The result limit is also an upper bound on caller-supplied limits.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ToolSearchConfig {
    /// Default and maximum number of search results (must be positive).
    pub max_results: usize,
    /// Catalog tool names retained in listings and excluded from search.
    pub always_visible: Vec<String>,
    /// Name of the synthetic search tool.
    pub search_tool_name: String,
    /// Name of the synthetic dispatch tool.
    pub call_tool_name: String,
    /// Include input-schema parameter names and descriptions in the index.
    pub index_parameters: bool,
}

impl Default for ToolSearchConfig {
    fn default() -> Self {
        Self {
            max_results: 5,
            always_visible: Vec::new(),
            search_tool_name: "search_tools".into(),
            call_tool_name: "call_tool".into(),
            index_parameters: true,
        }
    }
}

/// Wrap any `ServerHandler`, including one backed by a `ToolRouter`.
///
/// The underlying catalog is fetched (including all pages) for every search/list,
/// so dynamic and request-specific catalogs remain current. The BM25 index is
/// built lazily and reused only when the searchable definitions are unchanged.
/// Outgoing `tools/list_changed` notifications need no additional invalidation hook.
/// Original tools remain directly callable; this wrapper does not grant access.
#[derive(Debug)]
pub struct ToolSearchServer<H> {
    inner: H,
    config: ToolSearchConfig,
    index: Mutex<ToolSearchIndex>,
}

impl<H> Deref for ToolSearchServer<H> {
    type Target = H;

    fn deref(&self) -> &H {
        &self.inner
    }
}

impl<H: ServerHandler> ToolSearchServer<H> {
    /// Use the default synthetic names and a maximum of five results.
    pub fn new(inner: H) -> Self {
        Self::with_config(inner, ToolSearchConfig::default())
            .expect("default tool search configuration is valid")
    }

    /// Validate settings. Catalog name collisions are rejected when listing/searching.
    pub fn with_config(inner: H, config: ToolSearchConfig) -> Result<Self, ErrorData> {
        let valid_name = |name: &str| {
            !name.is_empty()
                && name.len() <= 128
                && name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
        };
        if config.max_results == 0
            || !valid_name(&config.search_tool_name)
            || !valid_name(&config.call_tool_name)
            || config.search_tool_name == config.call_tool_name
            || config
                .always_visible
                .iter()
                .any(|name| name == &config.search_tool_name || name == &config.call_tool_name)
        {
            return Err(ErrorData::invalid_params(
                "invalid tool search configuration",
                None,
            ));
        }
        let index = Mutex::new(ToolSearchIndex::with_parameter_indexing(
            Vec::new(),
            config.index_parameters,
        ));
        Ok(Self {
            inner,
            config,
            index,
        })
    }

    async fn catalog(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let mut result = self.inner.list_tools(None, context.clone()).await?;
        let mut cursors = HashSet::new();
        while let Some(cursor) = result.next_cursor.take() {
            if !cursors.insert(cursor.clone()) {
                return Err(ErrorData::internal_error(
                    "tool catalog repeated a pagination cursor",
                    None,
                ));
            }
            let page = self
                .inner
                .list_tools(
                    Some(PaginatedRequestParams {
                        cursor: Some(cursor),
                        ..Default::default()
                    }),
                    context.clone(),
                )
                .await?;
            result.tools.extend(page.tools);
            result.next_cursor = page.next_cursor;
        }
        if result
            .tools
            .iter()
            .any(|tool| self.is_synthetic(&tool.name))
        {
            return Err(ErrorData::invalid_params(
                "synthetic tool name collides with catalog; configure different names",
                None,
            ));
        }
        Ok(result)
    }

    fn is_synthetic(&self, name: &str) -> bool {
        name == self.config.search_tool_name || name == self.config.call_tool_name
    }

    fn synthetic_tools(&self) -> [Tool; 2] {
        let mut search = Tool::new(
            self.config.search_tool_name.clone(),
            "Search for tools using a natural-language query. Returns full tool definitions; call them directly or via the dispatch tool.",
            json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "max_results": {"type": "integer", "minimum": 1, "maximum": self.config.max_results, "default": self.config.max_results}
                },
                "required": ["query"],
                "additionalProperties": false
            }).as_object().unwrap().clone(),
        );
        search.annotations = Some(ToolAnnotations::new().read_only(true));
        let call = Tool::new(
            self.config.call_tool_name.clone(),
            "Call a discovered tool by name, passing its arguments.",
            json!({
                "type": "object",
                "properties": {"name": {"type": "string"}, "arguments": {"type": "object"}},
                "required": ["name"],
                "additionalProperties": false
            })
            .as_object()
            .unwrap()
            .clone(),
        );
        [search, call]
    }

    pub(crate) fn search_get_tool(&self, name: &str) -> Option<Tool> {
        self.synthetic_tools()
            .into_iter()
            .find(|tool| tool.name == name)
            .or_else(|| self.inner.get_tool(name))
    }

    pub(crate) async fn search_list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        if request.and_then(|request| request.cursor).is_some() {
            return Err(ErrorData::invalid_params(
                "tool search listing has no pagination cursor",
                None,
            ));
        }
        let mut result = self.catalog(context).await?;
        result
            .tools
            .retain(|tool| self.config.always_visible.contains(&tool.name.to_string()));
        result.tools.extend(self.synthetic_tools());
        Ok(result)
    }

    pub(crate) async fn search_call_tool(
        &self,
        mut request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if request.name == self.config.search_tool_name {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Search {
                query: String,
                max_results: Option<usize>,
            }
            let args: Search = serde_json::from_value(json!(request.arguments))
                .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
            let limit = args.max_results.unwrap_or(self.config.max_results);
            if limit == 0 || limit > self.config.max_results {
                return Err(ErrorData::invalid_params(
                    "max_results is outside the configured range",
                    None,
                ));
            }
            let mut tools = self.catalog(context).await?.tools;
            tools.retain(|tool| !self.config.always_visible.contains(&tool.name.to_string()));
            let mut index = self.index.lock().await;
            if index.tools() != tools {
                index.replace_tools(tools);
            }
            let tools: Vec<_> = index
                .search(&args.query, limit)
                .into_iter()
                .map(|hit| hit.tool)
                .collect();
            return Ok(CallToolResult::structured(json!({ "tools": tools })).into());
        }
        if request.name == self.config.call_tool_name {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Call {
                name: String,
                arguments: Option<JsonObject>,
            }
            let args: Call = serde_json::from_value(json!(request.arguments))
                .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
            if self.is_synthetic(&args.name) {
                return Err(ErrorData::invalid_params(
                    "cannot dispatch a synthetic tool",
                    None,
                ));
            }
            request.name = args.name.into();
            request.arguments = args.arguments;
        }
        self.inner.call_tool(request, context).await
    }
}
