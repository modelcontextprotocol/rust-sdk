# RMCP Feature Documentation

This document covers the core MCP features supported by `rmcp`, with server and client code examples for each.

For the full MCP specification, see [modelcontextprotocol.io](https://modelcontextprotocol.io/specification/2025-11-25).

## Table of Contents

- [Resources](#resources)
- [Prompts](#prompts)
- [Sampling](#sampling)
- [Roots](#roots)
- [Logging](#logging)
- [Completions](#completions)
- [Notifications](#notifications)
- [Subscriptions](#subscriptions)

---

## Resources

Resources let servers expose data (files, database records, API responses) that clients can read. Each resource is identified by a URI and returns content as text or binary (base64-encoded) data. Resource templates allow servers to declare URI patterns with dynamic parameters.

**MCP Spec:** [Resources](https://modelcontextprotocol.io/specification/2025-11-25/server/resources)

### Server-side

Implement `list_resources()`, `read_resource()`, and optionally `list_resource_templates()` on the `ServerHandler` trait. Enable the resources capability in `get_info()`.

```rust
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
    model::*,
    service::RequestContext,
    transport::stdio,
};
use serde_json::json;

#[derive(Clone)]
struct MyServer;

impl ServerHandler for MyServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder()
                .enable_resources()
                .build(),
            ..Default::default()
        }
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult {
            resources: vec![
                RawResource::new("file:///config.json", "config").no_annotation(),
                RawResource::new("memo://insights", "insights").no_annotation(),
            ],
            next_cursor: None,
            meta: None,
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        match request.uri.as_str() {
            "file:///config.json" => Ok(ReadResourceResult {
                contents: vec![ResourceContents::text(r#"{"key": "value"}"#, &request.uri)],
            }),
            "memo://insights" => Ok(ReadResourceResult {
                contents: vec![ResourceContents::text("Analysis results...", &request.uri)],
            }),
            _ => Err(McpError::resource_not_found(
                "resource_not_found",
                Some(json!({ "uri": request.uri })),
            )),
        }
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        Ok(ListResourceTemplatesResult {
            resource_templates: vec![],
            next_cursor: None,
            meta: None,
        })
    }
}
```

### Client-side

```rust
use rmcp::model::{ReadResourceRequestParams};

// List all resources (handles pagination automatically)
let resources = client.list_all_resources().await?;

// Read a specific resource by URI
let result = client.read_resource(ReadResourceRequestParams {
    meta: None,
    uri: "file:///config.json".into(),
}).await?;

// List resource templates
let templates = client.list_all_resource_templates().await?;
```

### Notifications

Servers can notify clients when the resource list changes or when a specific resource is updated:

```rust
// Notify that the resource list has changed (clients should re-fetch)
context.peer.notify_resource_list_changed().await?;

// Notify that a specific resource was updated
context.peer.notify_resource_updated(ResourceUpdatedNotificationParam {
    uri: "file:///config.json".into(),
}).await?;
```

Clients handle these via `ClientHandler`:

```rust
impl ClientHandler for MyClient {
    async fn on_resource_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) {
        // Re-fetch the resource list
    }

    async fn on_resource_updated(
        &self,
        params: ResourceUpdatedNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        // Re-read the updated resource at params.uri
    }
}
```

**Example:** [`examples/servers/src/common/counter.rs`](../examples/servers/src/common/counter.rs) (server), [`examples/clients/src/everything_stdio.rs`](../examples/clients/src/everything_stdio.rs) (client)

---

## Prompts

Prompts are reusable message templates that servers expose to clients. They accept typed arguments and return conversation messages. The `#[prompt]` macro handles argument validation and routing automatically.

**MCP Spec:** [Prompts](https://modelcontextprotocol.io/specification/2025-11-25/server/prompts)

### Server-side

Use the `#[prompt_router]`, `#[prompt]`, and `#[prompt_handler]` macros to define prompts declaratively. Arguments are defined as structs deriving `JsonSchema`.

```rust
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
    handler::server::{router::prompt::PromptRouter, wrapper::Parameters},
    model::*,
    prompt, prompt_handler, prompt_router,
    schemars::JsonSchema,
    service::RequestContext,
    transport::stdio,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CodeReviewArgs {
    #[schemars(description = "Programming language of the code")]
    pub language: String,
    #[schemars(description = "Focus areas for the review")]
    pub focus_areas: Option<Vec<String>>,
}

#[derive(Clone)]
pub struct MyServer {
    prompt_router: PromptRouter<Self>,
}

#[prompt_router]
impl MyServer {
    fn new() -> Self {
        Self { prompt_router: Self::prompt_router() }
    }

    /// Simple prompt without parameters
    #[prompt(name = "greeting", description = "A simple greeting")]
    async fn greeting(&self) -> Vec<PromptMessage> {
        vec![PromptMessage::new_text(
            PromptMessageRole::User,
            "Hello! How can you help me today?",
        )]
    }

    /// Prompt with typed arguments
    #[prompt(name = "code_review", description = "Review code in a given language")]
    async fn code_review(
        &self,
        Parameters(args): Parameters<CodeReviewArgs>,
    ) -> Result<GetPromptResult, McpError> {
        let focus = args.focus_areas
            .unwrap_or_else(|| vec!["correctness".into()]);

        Ok(GetPromptResult {
            description: Some(format!("Code review for {}", args.language)),
            messages: vec![
                PromptMessage::new_text(
                    PromptMessageRole::User,
                    format!("Review my {} code. Focus on: {}", args.language, focus.join(", ")),
                ),
            ],
        })
    }
}

#[prompt_handler]
impl ServerHandler for MyServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_prompts().build(),
            ..Default::default()
        }
    }
}
```

Prompt functions support several return types:
- `Vec<PromptMessage>` -- simple message list
- `GetPromptResult` -- messages with an optional description
- `Result<T, McpError>` -- either of the above, with error handling

### Client-side

```rust
use rmcp::model::GetPromptRequestParams;

// List all prompts
let prompts = client.list_all_prompts().await?;

// Get a prompt with arguments
let result = client.get_prompt(GetPromptRequestParams {
    meta: None,
    name: "code_review".into(),
    arguments: Some(rmcp::object!({
        "language": "Rust",
        "focus_areas": ["performance", "safety"]
    })),
}).await?;
```

### Notifications

```rust
// Server: notify that available prompts have changed
context.peer.notify_prompt_list_changed().await?;
```

**Example:** [`examples/servers/src/prompt_stdio.rs`](../examples/servers/src/prompt_stdio.rs) (server), [`examples/clients/src/everything_stdio.rs`](../examples/clients/src/everything_stdio.rs) (client)

---

## Sampling

Sampling flips the usual direction: the server asks the client to run an LLM completion. The server sends a `create_message` request, the client processes it through its LLM, and returns the result.

**MCP Spec:** [Sampling](https://modelcontextprotocol.io/specification/2025-11-25/client/sampling)

### Server-side (requesting sampling)

Access the client's sampling capability through `context.peer.create_message()`:

```rust
use rmcp::model::*;

// Inside a ServerHandler method (e.g., call_tool):
let response = context.peer.create_message(CreateMessageRequestParams {
    meta: None,
    task: None,
    messages: vec![SamplingMessage::user_text("Explain this error: connection refused")],
    model_preferences: Some(ModelPreferences {
        hints: Some(vec![ModelHint { name: Some("claude".into()) }]),
        cost_priority: Some(0.3),
        speed_priority: Some(0.8),
        intelligence_priority: Some(0.7),
    }),
    system_prompt: Some("You are a helpful assistant.".into()),
    include_context: Some(ContextInclusion::None),
    temperature: Some(0.7),
    max_tokens: 150,
    stop_sequences: None,
    metadata: None,
    tools: None,
    tool_choice: None,
}).await?;

// Extract the response text
let text = response.message.content
    .first()
    .and_then(|c| c.as_text())
    .map(|t| &t.text);
```

### Client-side (handling sampling)

On the client side, implement `ClientHandler::create_message()`. This is where you'd call your actual LLM:

```rust
use rmcp::{ClientHandler, model::*, service::{RequestContext, RoleClient}};

#[derive(Clone, Default)]
struct MyClient;

impl ClientHandler for MyClient {
    async fn create_message(
        &self,
        params: CreateMessageRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<CreateMessageResult, ErrorData> {
        // Forward to your LLM, or return a mock response:
        let response_text = call_your_llm(&params.messages).await;

        Ok(CreateMessageResult {
            message: SamplingMessage::assistant_text(response_text),
            model: "my-model".into(),
            stop_reason: Some(CreateMessageResult::STOP_REASON_END_TURN.into()),
        })
    }
}
```

**Example:** [`examples/servers/src/sampling_stdio.rs`](../examples/servers/src/sampling_stdio.rs) (server), [`examples/clients/src/sampling_stdio.rs`](../examples/clients/src/sampling_stdio.rs) (client)

---

## Roots

Roots tell servers which directories or projects the client is working in. A root is a URI (typically `file://`) pointing to a workspace or repository. Servers can query roots to know where to look for files and how to scope their work.

**MCP Spec:** [Roots](https://modelcontextprotocol.io/specification/2025-11-25/client/roots)

### Server-side

Ask the client for its root list, and handle change notifications:

```rust
use rmcp::{ServerHandler, model::*, service::{NotificationContext, RoleServer}};

impl ServerHandler for MyServer {
    // Query the client for its roots
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let roots = context.peer.list_roots().await?;
        // Use roots.roots to understand workspace boundaries
        // ...
    }

    // Called when the client's root list changes
    async fn on_roots_list_changed(
        &self,
        _context: NotificationContext<RoleServer>,
    ) {
        // Re-fetch roots to stay current
    }
}
```

### Client-side

Clients declare roots capability and implement `list_roots()`:

```rust
use rmcp::{ClientHandler, model::*};

impl ClientHandler for MyClient {
    async fn list_roots(
        &self,
        _context: RequestContext<RoleClient>,
    ) -> Result<ListRootsResult, ErrorData> {
        Ok(ListRootsResult {
            roots: vec![
                Root {
                    uri: "file:///home/user/project".into(),
                    name: Some("My Project".into()),
                },
            ],
        })
    }
}
```

Clients notify the server when roots change:

```rust
// After adding or removing a workspace root:
client.notify_roots_list_changed().await?;
```

---

## Logging

Servers can send structured log messages to clients. The client sets a minimum severity level, and the server sends messages through the peer notification interface.

**MCP Spec:** [Logging](https://modelcontextprotocol.io/specification/2025-11-25/server/utilities/logging)

### Server-side

Enable the logging capability, handle level changes from the client, and send log messages via the peer:

```rust
use rmcp::{ServerHandler, model::*, service::RequestContext};

impl ServerHandler for MyServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder()
                .enable_logging()
                .build(),
            ..Default::default()
        }
    }

    // Client sets the minimum log level
    async fn set_level(
        &self,
        request: SetLevelRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        // Store request.level and filter future log messages accordingly
        Ok(())
    }
}

// Send a log message from any handler with access to the peer:
context.peer.notify_logging_message(LoggingMessageNotificationParam {
    level: LoggingLevel::Info,
    logger: Some("my-server".into()),
    data: serde_json::json!({
        "message": "Processing completed",
        "items_processed": 42
    }),
}).await?;
```

Available log levels (from least to most severe): `Debug`, `Info`, `Notice`, `Warning`, `Error`, `Critical`, `Alert`, `Emergency`.

### Client-side

Clients handle incoming log messages via `ClientHandler`:

```rust
impl ClientHandler for MyClient {
    async fn on_logging_message(
        &self,
        params: LoggingMessageNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        println!("[{}] {}: {}", params.level,
            params.logger.unwrap_or_default(), params.data);
    }
}
```

Clients can also set the server's log level:

```rust
client.set_level(SetLevelRequestParams {
    level: LoggingLevel::Warning,
    meta: None,
}).await?;
```

---

## Completions

Completions give auto-completion suggestions for prompt or resource template arguments. As a user fills in arguments, the client can ask the server for suggestions based on what's already been entered.

**MCP Spec:** [Completions](https://modelcontextprotocol.io/specification/2025-11-25/server/utilities/completion)

### Server-side

Enable the completions capability and implement the `complete()` handler. Use `request.context` to inspect previously filled arguments:

```rust
use rmcp::{ErrorData as McpError, ServerHandler, model::*, service::RequestContext, RoleServer};

impl ServerHandler for MyServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder()
                .enable_completions()
                .enable_prompts()
                .build(),
            ..Default::default()
        }
    }

    async fn complete(
        &self,
        request: CompleteRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CompleteResult, McpError> {
        let values = match &request.r#ref {
            Reference::Prompt(prompt_ref) if prompt_ref.name == "sql_query" => {
                match request.argument.name.as_str() {
                    "operation" => vec!["SELECT", "INSERT", "UPDATE", "DELETE"],
                    "table" => vec!["users", "orders", "products"],
                    "columns" => {
                        // Adapt suggestions based on previously filled arguments
                        if let Some(ctx) = &request.context {
                            if let Some(op) = ctx.get_argument("operation") {
                                match op.to_uppercase().as_str() {
                                    "SELECT" | "UPDATE" => {
                                        vec!["id", "name", "email", "created_at"]
                                    }
                                    _ => vec![],
                                }
                            } else { vec![] }
                        } else { vec![] }
                    }
                    _ => vec![],
                }
            }
            _ => vec![],
        };

        // Filter by the user's partial input
        let filtered: Vec<String> = values.into_iter()
            .map(String::from)
            .filter(|v| v.to_lowercase().contains(&request.argument.value.to_lowercase()))
            .collect();

        Ok(CompleteResult {
            completion: CompletionInfo {
                values: filtered,
                total: None,
                has_more: Some(false),
            },
        })
    }
}
```

### Client-side

```rust
use rmcp::model::*;

let result = client.complete(CompleteRequestParams {
    meta: None,
    r#ref: Reference::Prompt(PromptReference {
        name: "sql_query".into(),
    }),
    argument: ArgumentInfo {
        name: "operation".into(),
        value: "SEL".into(),
    },
    context: None,
}).await?;

// result.completion.values contains suggestions like ["SELECT"]
```

**Example:** [`examples/servers/src/completion_stdio.rs`](../examples/servers/src/completion_stdio.rs)

---

## Notifications

Notifications are fire-and-forget messages -- no response is expected. They cover progress updates, cancellation, and lifecycle events. Both sides can send and receive them.

**MCP Spec:** [Notifications](https://modelcontextprotocol.io/specification/2025-11-25/basic/notifications)

### Progress notifications

Servers can report progress during long-running operations:

```rust
use rmcp::model::*;

// Inside a tool handler:
for i in 0..total_items {
    process_item(i).await;

    context.peer.notify_progress(ProgressNotificationParam {
        progress_token: ProgressToken(NumberOrString::Number(i as i64)),
        progress: i as f64,
        total: Some(total_items as f64),
        message: Some(format!("Processing item {}/{}", i + 1, total_items)),
    }).await?;
}
```

### Cancellation

Either side can cancel an in-progress request:

```rust
// Send a cancellation
context.peer.notify_cancelled(CancelledNotificationParam {
    request_id: the_request_id,
    reason: Some("User requested cancellation".into()),
}).await?;
```

Handle cancellation in `ServerHandler` or `ClientHandler`:

```rust
impl ServerHandler for MyServer {
    async fn on_cancelled(
        &self,
        params: CancelledNotificationParam,
        _context: NotificationContext<RoleServer>,
    ) {
        // Abort work for params.request_id
    }
}
```

### Initialized notification

Clients send `initialized` after the handshake completes:

```rust
// Sent automatically by rmcp during the serve() handshake.
// Servers handle it via:
impl ServerHandler for MyServer {
    async fn on_initialized(
        &self,
        _context: NotificationContext<RoleServer>,
    ) {
        // Server is ready to receive requests
    }
}
```

### List-changed notifications

When available tools, prompts, or resources change, tell the client:

```rust
context.peer.notify_tool_list_changed().await?;
context.peer.notify_prompt_list_changed().await?;
context.peer.notify_resource_list_changed().await?;
```

**Example:** [`examples/servers/src/common/progress_demo.rs`](../examples/servers/src/common/progress_demo.rs)

---

## Subscriptions

Clients can subscribe to specific resources. When a subscribed resource changes, the server sends a notification and the client can re-read it.

**MCP Spec:** [Resources - Subscriptions](https://modelcontextprotocol.io/specification/2025-11-25/server/resources#subscriptions)

### Server-side

Enable subscriptions in the resources capability and implement the `subscribe()` / `unsubscribe()` handlers:

```rust
use rmcp::{ErrorData as McpError, ServerHandler, model::*, service::RequestContext, RoleServer};
use std::sync::Arc;
use tokio::sync::Mutex;
use std::collections::HashSet;

#[derive(Clone)]
struct MyServer {
    subscriptions: Arc<Mutex<HashSet<String>>>,
}

impl ServerHandler for MyServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder()
                .enable_resources()
                .enable_resources_subscribe()
                .build(),
            ..Default::default()
        }
    }

    async fn subscribe(
        &self,
        request: SubscribeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        self.subscriptions.lock().await.insert(request.uri);
        Ok(())
    }

    async fn unsubscribe(
        &self,
        request: UnsubscribeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        self.subscriptions.lock().await.remove(&request.uri);
        Ok(())
    }
}
```

When a subscribed resource changes, notify the client:

```rust
// Check if the resource has subscribers, then notify
context.peer.notify_resource_updated(ResourceUpdatedNotificationParam {
    uri: "file:///config.json".into(),
}).await?;
```

### Client-side

```rust
use rmcp::model::*;

// Subscribe to updates for a resource
client.subscribe(SubscribeRequestParams {
    meta: None,
    uri: "file:///config.json".into(),
}).await?;

// Unsubscribe when no longer needed
client.unsubscribe(UnsubscribeRequestParams {
    meta: None,
    uri: "file:///config.json".into(),
}).await?;
```

Handle update notifications in `ClientHandler`:

```rust
impl ClientHandler for MyClient {
    async fn on_resource_updated(
        &self,
        params: ResourceUpdatedNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        // Re-read the resource at params.uri
    }
}
```
