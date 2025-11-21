//! Demonstration how to use enum selection in elicitation forms.
//!
//! This example server allows users to select enum values via elicitation forms.
//! To work with enum inference, it is required to use specific `schemars` attributes and apply some workarounds:
//! - Use `#[schemars(inline)]` to ensure the enum is inlined in the schema.
//! - Use `#[schemars(extend("type" = "string"))]` to manually add the required type field, since `schemars` does not provide it for enums.
//! - Optionally, use `#[schemars(title = "...")]` to provide titles for enum variants.
//! For more details, see: https://docs.rs/schemars/latest/schemars/
use std::{
    fmt::{Display, Formatter},
    sync::Arc,
};

use rmcp::{
    ErrorData as McpError, ServerHandler, elicit_safe,
    handler::server::router::tool::ToolRouter,
    model::*,
    service::{RequestContext, RoleServer},
    tool, tool_handler, tool_router,
    transport::{
        StreamableHttpService, streamable_http_server::session::local::LocalSessionManager,
    },
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

const BIND_ADDRESS: &str = "127.0.0.1:8000";

#[derive(Debug, Serialize, Deserialize, JsonSchema, Default)]
// inline attribute required to work for schema inference in elicitation forms
#[schemars(inline)]
// schemars does not provide required type field for enums, so we extend it here
#[schemars(extend("type" = "string"))]
enum TitledEnum {
    #[schemars(title = "Title for the first value")]
    #[default]
    FirstValue,
    #[schemars(title = "Title for the second value")]
    SecondValue,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
// inline attribute required to work for schema inference in elicitation forms
#[schemars(inline)]
enum UntitledEnum {
    First,
    Second,
    Third,
}

fn default_untitled_multi_select() -> Vec<UntitledEnum> {
    vec![UntitledEnum::Second, UntitledEnum::Third]
}
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "User information")]
struct SelectEnumForm {
    pub single_select_untitled: UntitledEnum,
    #[schemars(
        title = "Single Select Titled",
        description = "Description for single select enum",
        default
    )]
    pub single_select_titled: TitledEnum,
    #[serde(default = "default_untitled_multi_select")]
    pub multi_select_untitled: Vec<UntitledEnum>,
    #[schemars(
        title = "Multi Select Titled",
        description = "Multi Select Description"
    )]
    pub multi_select_titled: Vec<TitledEnum>,
}

impl Display for SelectEnumForm {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = format!(
            "Current Selections:\n\
                Single Select Untitled: {:?}\n\
                Single Select Titled: {:?}\n\
            Multi Select Untitled: {:?}\n\
            Multi Select Titled: {:?}\n",
            self.single_select_untitled,
            self.single_select_titled,
            self.multi_select_untitled,
            self.multi_select_titled,
        );
        write!(f, "{s}")
    }
}

elicit_safe!(SelectEnumForm);

#[derive(Clone)]
struct ElicitationEnumFormServer {
    selection: Arc<Mutex<SelectEnumForm>>,
    tool_router: ToolRouter<ElicitationEnumFormServer>,
}

#[tool_router]
impl ElicitationEnumFormServer {
    pub fn new() -> Self {
        Self {
            selection: Arc::new(Mutex::new(SelectEnumForm {
                single_select_untitled: UntitledEnum::First,
                single_select_titled: TitledEnum::FirstValue,
                multi_select_untitled: vec![UntitledEnum::Second],
                multi_select_titled: vec![TitledEnum::SecondValue],
            })),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Get current enum selection form")]
    async fn get_enum_form(&self) -> Result<CallToolResult, McpError> {
        let guard = self.selection.lock().await;
        Ok(CallToolResult::success(vec![Content::text(format!(
            "{}",
            *guard
        ))]))
    }

    #[tool(description = "Set enum selection via elicitation form")]
    async fn set_enum_form(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        match context
            .peer
            .elicit::<SelectEnumForm>("Please provide your selection".to_string())
            .await
        {
            Ok(Some(form)) => {
                let mut guard = self.selection.lock().await;
                *guard = form;
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "Updated Selection:\n{}",
                    *guard
                ))]))
            }
            Ok(None) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    "Elicitation cancelled by user.",
                )]));
            }
            Err(err) => {
                return Err(McpError::internal_error(
                    format!("Elicitation failed: {err}"),
                    None,
                ));
            }
        }
    }
}

#[tool_handler]
impl ServerHandler for ElicitationEnumFormServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "Simple server demonstrating elicitation for enum selection".to_string(),
            ),
            ..Default::default()
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "debug".to_string().into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let service = StreamableHttpService::new(
        || Ok(ElicitationEnumFormServer::new()),
        LocalSessionManager::default().into(),
        Default::default(),
    );

    let router = axum::Router::new().nest_service("/mcp", service);
    let tcp_listener = tokio::net::TcpListener::bind(BIND_ADDRESS).await?;
    let _ = axum::serve(tcp_listener, router)
        .with_graceful_shutdown(async { tokio::signal::ctrl_c().await.unwrap() })
        .await;
    Ok(())
}
