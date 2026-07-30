# RMCP
[![Crates.io Version](https://img.shields.io/crates/v/rmcp)](https://crates.io/crates/rmcp)
[![docs.rs](https://img.shields.io/docsrs/rmcp)](https://docs.rs/rmcp/latest/rmcp)
[![CI](https://github.com/modelcontextprotocol/rust-sdk/actions/workflows/ci.yml/badge.svg)](https://github.com/modelcontextprotocol/rust-sdk/actions/workflows/ci.yml)
[![License](https://img.shields.io/crates/l/rmcp)](LICENSE)

An official Rust Model Context Protocol SDK implementation with tokio async runtime.

> **Migrating to 3.x?** See the [migration guide](https://github.com/modelcontextprotocol/rust-sdk/discussions/969) for breaking changes and upgrade instructions.

This repository contains the following crates:

- [rmcp](crates/rmcp): The core crate providing the RMCP protocol implementation - see [rmcp](crates/rmcp/README.md)
- [rmcp-macros](crates/rmcp-macros): A procedural macro crate for generating RMCP tool implementations - see [rmcp-macros](crates/rmcp-macros/README.md)

This SDK implements the stable MCP **`2026-07-28`** specification while
remaining fully compatible with the **`2025-11-25`** release and earlier
versions. Features introduced in `2026-07-28` — server discovery & negotiation,
transport-neutral subscriptions, long-running tasks, response caching,
multi-round-trip requests, and standard HTTP routing headers — are documented
below. For the full MCP specification, see
[modelcontextprotocol.io](https://modelcontextprotocol.io/specification/2026-07-28).

## Table of Contents

- [Usage](#usage)
- [Tools](#tools)
- [Resources](#resources)
- [Prompts](#prompts)
- [Sampling](#sampling)
- [Elicitation](#elicitation)
- [Roots](#roots)
- [Logging](#logging)
- [Completions](#completions)
- [Notifications](#notifications)
- [Subscriptions](#subscriptions)
- [Multi-Round-Trip Requests](#multi-round-trip-requests)
- [Tasks](#tasks-long-running-tool-invocations)
- [Caching](#caching)
- [Standard HTTP Headers](#standard-http-headers)
- [Stateless Streamable HTTP](#stateless-streamable-http)
- [Transports](#transports)
- [Pagination](#pagination)
- [Capability & Protocol Version Negotiation](#capability--protocol-version-negotiation)
- [JSON Schema 2020-12](#json-schema-2020-12)
- [Examples](#examples)
- [OAuth Support](#oauth-support)
- [Related Resources](#related-resources)
- [Related Projects](#related-projects)
- [Development](#development)

## Usage

### Import the crate

Add the latest published version with cargo:

```sh
cargo add rmcp --features server
```

Or use the dev channel:

```sh
cargo add rmcp --features server --git https://github.com/modelcontextprotocol/rust-sdk --branch main
```
### Third Dependencies

Basic dependencies:
- [tokio](https://github.com/tokio-rs/tokio)
- [serde](https://github.com/serde-rs/serde)
Json Schema generation (version 2020-12):
- [schemars](https://github.com/GREsau/schemars)

### Build a Client

<details>
<summary>Start a client</summary>

```rust, ignore
use rmcp::{ServiceExt, transport::{TokioChildProcess, ConfigureCommandExt}};
use tokio::process::Command;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = ().serve(TokioChildProcess::new(Command::new("npx").configure(|cmd| {
        cmd.arg("-y").arg("@modelcontextprotocol/server-everything");
    }))?).await?;
    Ok(())
}
```
</details>

### Client lifecycle modes

`serve()` uses the legacy MCP lifecycle: the client sends `initialize`, receives
the negotiated server information, and then sends `notifications/initialized`.
Use [`ClientServiceExt::serve_with_lifecycle`](crates/rmcp/src/service/client.rs) to
select another lifecycle explicitly:

```rust, ignore
use rmcp::{ClientInfo, ClientLifecycleMode, ClientServiceExt, ProtocolVersion};

// Start directly with server/discover and include client metadata on every request.
let client = ClientInfo::default()
    .serve_with_lifecycle(
        transport,
        ClientLifecycleMode::Discover {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        },
    )
    .await?;

// Or probe the discover lifecycle and fall back when a legacy server reports
// that server/discover is not implemented.
let client = ClientInfo::default()
    .serve_with_lifecycle(
        transport,
        ClientLifecycleMode::Auto {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            legacy_version: Some(ProtocolVersion::V_2025_11_25),
        },
    )
    .await?;
```

`ClientLifecycleMode::Initialize` is equivalent to the existing `serve()` behavior.
Discover startup does not send `notifications/initialized`; discovery completes
startup, and each subsequent request carries its protocol version, client
information, and capabilities in `_meta`.

### Build a Server

<details>
<summary>Build a transport</summary>

```rust, ignore
use tokio::io::{stdin, stdout};
let transport = (stdin(), stdout());
```

</details>

<details>
<summary>Build a service</summary>

You can easily build a service by using [`ServerHandler`](crates/rmcp/src/handler/server.rs) or [`ClientHandler`](crates/rmcp/src/handler/client.rs).

```rust, ignore
let service = common::counter::Counter::new();
```
</details>

<details>
<summary>Start the server</summary>

```rust, ignore
// this call will finish the initialization process
let server = service.serve(transport).await?;
```
</details>

<details>
<summary>Interact with the server</summary>

Once the server is initialized, you can send requests or notifications:

```rust, ignore
// request
let roots = server.list_roots().await?;

// or send notification
server.notify_cancelled(...).await?;
```
</details>

<details>
<summary>Waiting for service shutdown</summary>

```rust, ignore
let quit_reason = server.waiting().await?;
// or cancel it
let quit_reason = server.cancel().await?;
```
</details>

---

## Tools

Tools let servers expose callable functions to clients. Each tool has a name, description, and a JSON Schema for its parameters. Clients discover tools via `list_tools` and invoke them via `call_tool`.

**MCP Spec:** [Tools](https://modelcontextprotocol.io/specification/2026-07-28/server/tools)

### Server-side

The `#[tool]`, `#[tool_router]`, and `#[tool_handler]` macros handle all the wiring. For a tools-only server you can use `#[tool_router(server_handler)]` to skip the separate `ServerHandler` impl:

```rust,ignore
use rmcp::{handler::server::wrapper::Parameters, schemars, tool, tool_router, ServiceExt, transport::stdio};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct AddParams {
    a: i32,
    b: i32,
}

#[derive(Clone)]
struct Calculator;

#[tool_router(server_handler)]
impl Calculator {
    #[tool(description = "Add two numbers")]
    fn add(&self, Parameters(AddParams { a, b }): Parameters<AddParams>) -> String {
        (a + b).to_string()
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let service = Calculator.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
```

The generated tool `inputSchema` and `outputSchema` are derived from the fields of `T`. The type name and documentation on `T` are ignored; only field names, field types, and field documentation are used.

> **`2026-07-28` (SEP-2106):** `outputSchema` may now be any JSON Schema type
> (not just `object`), and a tool result's `structuredContent` may be any JSON
> value (string, array, number, …) rather than only an object. Existing
> object-typed tools are unaffected.

When you need custom server metadata or multiple capabilities (tools + prompts), use explicit `#[tool_handler]`:

```rust,ignore
use rmcp::{handler::server::wrapper::Parameters, schemars, tool, tool_router, tool_handler, ServerHandler, ServiceExt};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct AddParams {
    a: i32,
    b: i32,
}

#[derive(Clone)]
struct Calculator;

#[tool_router]
impl Calculator {
    #[tool(description = "Add two numbers")]
    fn add(&self, Parameters(AddParams { a, b }): Parameters<AddParams>) -> String {
        (a + b).to_string()
    }
}

#[tool_handler(name = "calculator", version = "1.0.0", instructions = "A simple calculator")]
impl ServerHandler for Calculator {}
```

See [`crates/rmcp-macros`](crates/rmcp-macros/README.md) for full macro documentation.

#### Tool result content types

Beyond a plain `String`, tools can return images, audio, embedded resources, and
mixed content. Build a `CallToolResult` from a `Vec<ContentBlock>`:

```rust,ignore
use rmcp::model::{CallToolResult, ContentBlock, ResourceContents};

#[tool(description = "Render a chart")]
async fn chart(&self) -> Result<CallToolResult, McpError> {
    let png_base64 = render_png(); // base64-encoded image bytes
    let wav_base64 = render_wav(); // base64-encoded audio bytes

    Ok(CallToolResult::success(vec![
        // Text
        ContentBlock::text("Here is your chart:"),
        // Image — base64 data + MIME type
        ContentBlock::image(png_base64, "image/png"),
        // Audio — base64 data + MIME type
        ContentBlock::audio(wav_base64, "audio/wav"),
        // Embedded resource — inline text (or ResourceContents::blob for binary)
        ContentBlock::resource(ResourceContents::text(
            "chart source data",
            "chart://last/data.csv",
        )),
    ]))
}
# fn render_png() -> String { String::new() }
# fn render_wav() -> String { String::new() }
```

Image and audio data are base64 strings with a MIME type. For embedded
resources, `ResourceContents::text(..)` inlines text and
`ResourceContents::blob(base64, uri)` inlines binary.

#### Error handling

Two failure modes, chosen by **whose problem it is**:

- **Tool-level error** — `Ok(CallToolResult::error(vec![...]))`. The tool ran but
  failed in a way the caller should see (no rows matched, upstream 500). The
  client renders your `content`, so the message reaches the user. Use this for
  almost every "the tool ran and didn't work" case.
- **Protocol error** — `Err(McpError)` with a JSON-RPC code (e.g.
  `McpError::invalid_params(..)`). Use this when the server can't route or process
  the request at all; clients render these opaquely, so the caller does **not**
  see your message.

```rust,ignore
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::ErrorData as McpError;

#[tool(description = "Look up a record")]
async fn lookup(&self, Parameters(args): Parameters<LookupArgs>) -> Result<CallToolResult, McpError> {
    // Malformed request — the server can't run anything → protocol error.
    if args.query.is_empty() {
        return Err(McpError::invalid_params("query must be non-empty", None));
    }

    // Tool ran, no result → tool-level error the user should see.
    let rows = self.run_query(&args.query).await;
    if rows.is_empty() {
        return Ok(CallToolResult::error(vec![ContentBlock::text(
            format!("no rows matched '{}'", args.query),
        )]));
    }

    Ok(CallToolResult::success(vec![ContentBlock::text(format_rows(&rows))]))
}
```

### Client-side

```rust,ignore
use rmcp::model::CallToolRequestParams;

// List all tools
let tools = client.list_all_tools().await?;

// Call a tool by name
let result = client.call_tool(CallToolRequestParams::new("add")).await?;
```

**Example:** [`examples/servers/src/common/calculator.rs`](examples/servers/src/common/calculator.rs) (server), [`examples/servers/src/calculator_stdio.rs`](examples/servers/src/calculator_stdio.rs) (stdio runner)

---

## Resources

Resources let servers expose data (files, database records, API responses) that clients can read. Each resource is identified by a URI and returns content as text or binary (base64-encoded) data. Resource templates allow servers to declare URI patterns with dynamic parameters.

**MCP Spec:** [Resources](https://modelcontextprotocol.io/specification/2026-07-28/server/resources)

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
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_resources()
                .build(),
        )
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult {
            resources: vec![
                Resource::new("file:///config.json", "config"),
                Resource::new("memo://insights", "insights"),
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
            "file:///config.json" => Ok(ReadResourceResult::new(vec![
                ResourceContents::text(r#"{"key": "value"}"#, &request.uri),
            ])),
            "memo://insights" => Ok(ReadResourceResult::new(vec![
                ResourceContents::text("Analysis results...", &request.uri),
            ])),
            // Binary resource — base64-encode the bytes and return a blob.
            "file:///logo.png" => {
                use base64::{Engine, prelude::BASE64_STANDARD};
                let bytes = std::fs::read("logo.png").unwrap_or_default();
                let blob = BASE64_STANDARD.encode(bytes);
                Ok(ReadResourceResult::new(vec![
                    ResourceContents::blob(blob, &request.uri)
                        .with_mime_type("image/png"),
                ]))
            }
            // Template-expanded URI — the client fills in `{user_id}` from the
            // `users://{user_id}/profile` template declared in
            // `list_resource_templates`, and the server reads the concrete URI.
            uri if uri.starts_with("users://") && uri.ends_with("/profile") => {
                let user_id = uri
                    .trim_start_matches("users://")
                    .trim_end_matches("/profile");
                Ok(ReadResourceResult::new(vec![ResourceContents::text(
                    format!(r#"{{"id": "{user_id}", "name": "User {user_id}"}}"#),
                    uri,
                )]))
            }
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
        // Declare a URI template with a `{user_id}` parameter. Clients expand it
        // (e.g. `users://42/profile`) and pass the concrete URI to `read_resource`.
        Ok(ListResourceTemplatesResult {
            resource_templates: vec![
                ResourceTemplate::new("users://{user_id}/profile", "user-profile"),
            ],
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
let result = client.read_resource(
    ReadResourceRequestParams::new("file:///config.json"),
).await?;

// List resource templates, then read a resource through one by expanding its
// parameters into a concrete URI (`users://{user_id}/profile` → `users://42/profile`).
let templates = client.list_all_resource_templates().await?;
let profile = client.read_resource(
    ReadResourceRequestParams::new("users://42/profile"),
).await?;
```

### Notifications

Servers can notify clients when the resource list changes or when a specific resource is updated:

```rust
// Notify that the resource list has changed (clients should re-fetch)
context.peer.notify_resource_list_changed().await?;

// Notify that a specific resource was updated
context.peer.notify_resource_updated(
    ResourceUpdatedNotificationParam::new("file:///config.json"),
).await?;
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

**Example:** [`examples/servers/src/common/counter.rs`](examples/servers/src/common/counter.rs) (server), [`examples/clients/src/everything_stdio.rs`](examples/clients/src/everything_stdio.rs) (client)

---

## Prompts

Prompts are reusable message templates that servers expose to clients. They accept typed arguments and return conversation messages. The `#[prompt]` macro handles argument validation and routing automatically.

**MCP Spec:** [Prompts](https://modelcontextprotocol.io/specification/2026-07-28/server/prompts)

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
            Role::User,
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

        Ok(GetPromptResult::new(vec![
            PromptMessage::new_text(
                Role::User,
                format!("Review my {} code. Focus on: {}", args.language, focus.join(", ")),
            ),
        ])
        .with_description(format!("Code review for {}", args.language)))
    }
}

#[prompt_handler]
impl ServerHandler for MyServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_prompts().build())
    }
}
```

Prompt functions support several return types:
- `Vec<PromptMessage>` -- simple message list
- `GetPromptResult` -- messages with an optional description
- `Result<T, McpError>` -- either of the above, with error handling

#### Image and embedded-resource content

A `PromptMessage` can also carry an image or embedded resource. Use the
dedicated constructors (image/audio require the `base64` feature):

```rust,ignore
use rmcp::model::{PromptMessage, Role};

// Image content — raw bytes are base64-encoded for you.
let screenshot: &[u8] = load_png();
let msg = PromptMessage::new_image(Role::User, screenshot, "image/png", None, None);

// Embedded resource — inline a text resource by URI. Pass `Some(text)` for a
// text resource, or `None` for a blob resource.
let msg = PromptMessage::new_resource(
    Role::User,
    "file:///spec.md".to_string(),
    Some("text/markdown".to_string()),
    Some("# Specification\n...".to_string()),
    None, None, None,
);
# fn load_png() -> &'static [u8] { &[] }
```

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

**Example:** [`examples/servers/src/prompt_stdio.rs`](examples/servers/src/prompt_stdio.rs) (server), [`examples/clients/src/everything_stdio.rs`](examples/clients/src/everything_stdio.rs) (client)

---

## Sampling

> **Deprecated (SEP-2577):** Sampling is deprecated and will be removed in a future release. It remains fully functional for now. See [SEP-2577](https://github.com/modelcontextprotocol/modelcontextprotocol/pull/2577).

Sampling flips the usual direction: the server asks the client to run an LLM completion. The server sends a `create_message` request, the client processes it through its LLM, and returns the result.

**MCP Spec:** [Sampling](https://modelcontextprotocol.io/specification/2026-07-28/client/sampling)

### Server-side (requesting sampling)

Access the client's sampling capability through `context.peer.create_message()`:

```rust
use rmcp::model::*;

// Inside a ServerHandler method (e.g., call_tool):
let response = context.peer.create_message(
    CreateMessageRequestParams::new(
        vec![SamplingMessage::user_text("Explain this error: connection refused")],
        150,
    )
    .with_model_preferences(
        ModelPreferences::new()
            .with_hints(vec![ModelHint::new("claude")])
            .with_cost_priority(0.3)
            .with_speed_priority(0.8)
            .with_intelligence_priority(0.7),
    )
    .with_system_prompt("You are a helpful assistant.")
    .with_include_context(ContextInclusion::None)
    .with_temperature(0.7),
).await?;

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

        Ok(CreateMessageResult::new(
            SamplingMessage::assistant_text(response_text),
            "my-model".into(),
        )
        .with_stop_reason(CreateMessageResult::STOP_REASON_END_TURN))
    }
}
```

**Example:** [`examples/servers/src/sampling_stdio.rs`](examples/servers/src/sampling_stdio.rs) (server), [`examples/clients/src/sampling_stdio.rs`](examples/clients/src/sampling_stdio.rs) (client)

---

## Elicitation

Elicitation lets a server pause mid-operation to ask the user for input, in one
of two modes: **form mode** (structured fields with a JSON Schema) or **URL
mode** (send the user to a web page and wait for completion).

**MCP Spec:** [Elicitation](https://modelcontextprotocol.io/specification/2026-07-28/client/elicitation)

### Server-side (form mode)

Define a struct deriving `JsonSchema`, mark it `elicit_safe!`, and call
`elicit::<T>()` on the peer. Schema validation, defaults, and enum choices all
come from the type.

```rust,ignore
use rmcp::{elicit_safe, model::*, service::{RequestContext, RoleServer}};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "User information")]
pub struct UserInfo {
    #[schemars(description = "User's name")]
    pub name: String,
    // Optional field; omitted if the user doesn't provide it.
    #[serde(default)]
    #[schemars(description = "Preferred greeting")]
    pub greeting: Option<String>,
}

// Whitelist the type for elicitation (schema-validated on both ends).
elicit_safe!(UserInfo);

#[tool(description = "Greet the user")]
async fn greet(&self, ctx: RequestContext<RoleServer>) -> Result<CallToolResult, McpError> {
    // Returns Ok(Some(UserInfo)) if the user accepts.
    // Decline and cancel are returned as ElicitationError variants.
    match ctx.peer.elicit::<UserInfo>("Please provide your name").await {
        Ok(Some(info)) => Ok(CallToolResult::success(vec![ContentBlock::text(
            format!("Hello, {}!", info.name),
        )])),
        Ok(None) => Ok(CallToolResult::success(vec![ContentBlock::text(
            "No name provided.",
        )])),
        Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(
            format!("Elicitation failed: {e}"),
        )])),
    }
}
```

#### Enum values

Enum fields become a choice list. `schemars` needs two hints to inline and type
the enum correctly:

```rust,ignore
#[derive(Debug, Serialize, Deserialize, JsonSchema, Default)]
#[schemars(inline)]                     // inline the enum into the parent schema
#[schemars(extend("type" = "string"))]  // schemars omits `type` for enums; add it
enum Priority {
    #[schemars(title = "Low priority")]
    #[default]
    Low,
    #[schemars(title = "High priority")]
    High,
}
```

See [`examples/servers/src/elicitation_enum_inference.rs`](examples/servers/src/elicitation_enum_inference.rs)
for single-select, multi-select, titled, and defaulted enum forms.

### Server-side (URL mode)

For flows a form can't capture (OAuth consent, a payment page), send the user to
a URL. `elicit_url` returns the user's `ElicitationAction` rather than typed data:

```rust,ignore
use rmcp::model::ElicitationAction;
use url::Url;

let action = ctx.peer.elicit_url(
    "Please complete setup in your browser",
    Url::parse("https://example.com/setup").unwrap(),
    "setup-123", // a unique elicitation id
).await?;

match action {
    ElicitationAction::Accept  => { /* user consented */ }
    ElicitationAction::Decline => { /* user declined */ }
    ElicitationAction::Cancel  => { /* user aborted */ }
}
```

### Client-side

Implement `ClientHandler::create_elicitation()`, matching on the request variant
to handle form vs. URL mode:

```rust,ignore
use rmcp::{ClientHandler, model::*, service::{RequestContext, RoleClient}};

impl ClientHandler for MyClient {
    async fn create_elicitation(
        &self,
        request: ElicitRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, ErrorData> {
        match request {
            ElicitRequestParams::FormElicitationParams { message, .. } => {
                // Show `message` + the requested schema, collect input, then:
                Ok(ElicitResult {
                    action: ElicitationAction::Accept,
                    content: Some(rmcp::object!({ "name": "Ada" })),
                    meta: None,
                })
            }
            ElicitRequestParams::UrlElicitationParams { url, .. } => {
                // Open `url`, wait for the user, then report the action.
                let _ = url;
                Ok(ElicitResult { action: ElicitationAction::Accept, content: None, meta: None })
            }
        }
    }
}
```

On completion the client sends a `notifications/elicitation/response`
notification to release the waiting server-side `elicit_url` call.

**Example:** [`examples/servers/src/elicitation_stdio.rs`](examples/servers/src/elicitation_stdio.rs) (form + URL), [`examples/servers/src/elicitation_enum_inference.rs`](examples/servers/src/elicitation_enum_inference.rs) (enum forms)

---

## Roots

> **Deprecated (SEP-2577):** Roots is deprecated and will be removed in a future release. It remains fully functional for now. See [SEP-2577](https://github.com/modelcontextprotocol/modelcontextprotocol/pull/2577).

Roots tell servers which directories or projects the client is working in. A root is a URI (typically `file://`) pointing to a workspace or repository. Servers can query roots to know where to look for files and how to scope their work.

**MCP Spec:** [Roots](https://modelcontextprotocol.io/specification/2026-07-28/client/roots)

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
        Ok(ListRootsResult::new(vec![
            Root::new("file:///home/user/project").with_name("My Project"),
        ]))
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

> **Deprecated (SEP-2577):** Logging is deprecated and will be removed in a future release. It remains fully functional for now. See [SEP-2577](https://github.com/modelcontextprotocol/modelcontextprotocol/pull/2577).

Servers can send structured log messages to clients. The client sets a minimum severity level, and the server sends messages through the peer notification interface.

**MCP Spec:** [Logging](https://modelcontextprotocol.io/specification/2026-07-28/server/utilities/logging)

### Server-side

Enable the logging capability, handle level changes from the client, and send log messages via the peer:

```rust
use rmcp::{ServerHandler, model::*, service::RequestContext};

impl ServerHandler for MyServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_logging()
                .build(),
        )
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
context.peer.notify_logging_message(
    LoggingMessageNotificationParam::new(
        LoggingLevel::Info,
        serde_json::json!({
            "message": "Processing completed",
            "items_processed": 42
        }),
    )
    .with_logger("my-server"),
).await?;
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
client.set_level(SetLevelRequestParams::new(LoggingLevel::Warning)).await?;
```

---

## Completions

Completions give auto-completion suggestions for prompt or resource template arguments. As a user fills in arguments, the client can ask the server for suggestions based on what's already been entered.

**MCP Spec:** [Completions](https://modelcontextprotocol.io/specification/2026-07-28/server/utilities/completion)

### Server-side

Enable the completions capability and implement the `complete()` handler. Use `request.context` to inspect previously filled arguments:

```rust
use rmcp::{ErrorData as McpError, ServerHandler, model::*, service::RequestContext, RoleServer};

impl ServerHandler for MyServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_completions()
                .enable_prompts()
                .build(),
        )
    }

    async fn complete(
        &self,
        request: CompleteRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CompleteResult, McpError> {
        let values = match &request.r#ref {
            // Completion for a prompt argument (`ref/prompt`).
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
            // Completion for a resource-template argument (`ref/resource`). The
            // `uri` identifies the template (e.g. `users://{user_id}/profile`)
            // and `argument.name` is the template variable being completed.
            Reference::Resource(resource_ref)
                if resource_ref.uri == "users://{user_id}/profile" =>
            {
                match request.argument.name.as_str() {
                    "user_id" => vec!["1", "2", "42"],
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

        let completion = CompletionInfo::with_pagination(filtered, None, false)
            .map_err(|e| McpError::internal_error(e, None))?;
        Ok(CompleteResult::new(completion))
    }
}
```

### Client-side

```rust
use rmcp::model::*;

// Completion for a prompt argument.
let result = client.complete(CompleteRequestParams::new(
    Reference::for_prompt("sql_query"),
    ArgumentInfo::new("operation", "SEL"),
)).await?;

// result.completion.values contains suggestions like ["SELECT"]

// Completion for a resource-template argument: reference the template by URI
// and complete one of its variables (`user_id`).
let resource_completion = client.complete(CompleteRequestParams::new(
    Reference::for_resource("users://{user_id}/profile"),
    ArgumentInfo::new("user_id", "4"),
)).await?;

// resource_completion.completion.values contains suggestions like ["42"]
```

**Example:** [`examples/servers/src/completion_stdio.rs`](examples/servers/src/completion_stdio.rs)

---

## Notifications

Notifications are fire-and-forget messages -- no response is expected. They cover progress updates, cancellation, and lifecycle events. Both sides can send and receive them.

**MCP Spec:** [Notifications](https://modelcontextprotocol.io/specification/2026-07-28/basic#notifications)

### Progress notifications

Servers can report progress during long-running operations:

```rust
use rmcp::model::*;

// Inside a tool handler:
for i in 0..total_items {
    process_item(i).await;

    context.peer.notify_progress(
        ProgressNotificationParam::new(
            ProgressToken(NumberOrString::Number(i as i64)),
            i as f64,
        )
        .with_total(total_items as f64)
        .with_message(format!("Processing item {}/{}", i + 1, total_items)),
    ).await?;
}
```

### Cancellation

Either side can cancel an in-progress request:

```rust
// Send a cancellation
context.peer.notify_cancelled(CancelledNotificationParam::new(
    Some(the_request_id),
    Some("User requested cancellation".into()),
)).await?;
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

Legacy clients send `initialized` after the `initialize` handshake completes.
Clients using `ClientLifecycleMode::Discover` do not send this notification:

```rust
// Sent automatically by rmcp during the legacy serve() handshake.
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

**Example:** [`examples/servers/src/common/progress_demo.rs`](examples/servers/src/common/progress_demo.rs)

---

## Subscriptions

Protocol `2026-07-28` replaces `resources/subscribe`, `resources/unsubscribe`, and
the standalone HTTP GET stream with the transport-neutral, long-lived
`subscriptions/listen` request. Each requested notification category is opt-in.

**MCP Spec:** [Subscriptions](https://modelcontextprotocol.io/specification/2026-07-28/basic/patterns/subscriptions)

### Server-side

Declare the notification capabilities you serve, return the accepted subset,
and use the filter-enforcing subscription sink:

```rust
use rmcp::{
    ErrorData, ServerHandler,
    model::*,
    service::SubscriptionContext,
};

impl ServerHandler for MyServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .build(),
        )
    }

    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        Some(requested.clone())
    }

    async fn listen(&self, context: SubscriptionContext) -> Result<(), ErrorData> {
        if context.accepted().tools_list_changed == Some(true) {
            context.sink().notify_tool_list_changed().await
                .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
        }
        context.cancelled().await;
        Ok(())
    }
}
```

The SDK intersects the handler's filter with the requested categories and the
capabilities advertised by `get_info()`. It sends the acknowledgment before
`listen`, tags every sink notification with the listen request ID, and rejects
categories or resource URIs outside the accepted filter.

### Client-side

```rust
use rmcp::model::*;

let mut subscription = client.listen(
    SubscriptionFilter::builder()
        .tools_list_changed()
        .resource_subscription("file:///config.json")
        .build(),
).await?;

println!("accepted: {:?}", subscription.acknowledged());
while let Some(notification) = subscription.next().await? {
    println!("notification: {notification:?}");
}

subscription.cancel().await?;
```

`listen()` buffers up to 64 notifications per subscription. Use
`listen_with_capacity()` to choose a different non-zero capacity; if a consumer
falls behind, `Subscription::end()` reports `SubscriptionEnd::Lagged`.

For older negotiated protocol versions, the deprecated `subscribe()` and
`unsubscribe()` APIs retain their legacy wire behavior. Modern Streamable HTTP
uses the listen POST response stream directly and does not use sessions, GET,
DELETE, or `Last-Event-ID`. After an abrupt transport close, call `listen`
again; subscription state is not resumed across HTTP or stdio reconnects.

See the
[modern subscription server](examples/servers/src/subscriptions_streamhttp.rs)
and [client](examples/clients/src/subscriptions_streamhttp.rs) examples.

---

## Multi-Round-Trip Requests

Protocol `2026-07-28` adds Multi-Round-Trip Requests (MRTR, SEP-2322): a server
can answer a `tools/call`, `prompts/get`, or `resources/read` with an
`InputRequiredResult` instead of a final result, asking the client to fulfill
one or more embedded server requests (elicitation, sampling, or roots) and then
retry. The exchange is stateless — the server carries its progress in an opaque
`requestState` that the client echoes back verbatim.

**MCP Spec:** [Multiple Round-Trip Requests](https://modelcontextprotocol.io/specification/2026-07-28/server/tools#multiple-round-trip-requests)

### Server-side

Return an `InputRequiredResult` via the outcome enum for the method
(`CallToolResponse`, `GetPromptResponse`, or `ReadResourceResponse`). The SDK
only forwards it to peers that negotiated `2026-07-28` or newer — older peers
get a protocol error instead.

```rust, ignore
async fn call_tool(&self, request: CallToolRequestParams, _ctx: RequestContext<RoleServer>)
    -> Result<CallToolResponse, ErrorData>
{
    match request.request_state {
        // First round: ask the client for input, seal progress into requestState.
        None => {
            let mut input_requests = InputRequests::new();
            input_requests.insert("city".into(), InputRequest::Elicitation(elicit_city()));
            let sealed = self.codec.seal_json(&json!({ "awaiting": "city" }))?;
            Ok(InputRequiredResult::new(Some(input_requests), Some(sealed)).into())
        }
        // Retry round: verify the echoed state, read the responses, finish.
        Some(sealed) => {
            let _state = self.codec.open_json(&sealed)
                .map_err(|_| ErrorData::invalid_params("tampered request state", None))?;
            let city = request.input_responses.as_ref()
                .and_then(|r| r.get("city"));
            Ok(CallToolResult::success(vec![ContentBlock::text("It is sunny.")]).into())
        }
    }
}
```

> **`requestState` is untrusted.** The client echoes it back verbatim, so a
> stateless server that stores meaningful data in it MUST verify integrity
> first. Enable the `request-state` feature and use `RequestStateCodec` to seal
> and open it (HMAC-tagged), or keep state server-side and use `requestState`
> only as an opaque handle.

### Client-side

The high-level `call_tool`, `get_prompt`, and `read_resource` helpers drive MRTR
automatically: they fulfill each embedded request through the local
`ClientHandler` and retry, up to `DEFAULT_MRTR_MAX_ROUNDS` (10).

```rust, ignore
// Auto mode: the SDK fulfills embedded requests and retries for you.
let result = client.call_tool(CallToolRequestParams::new("weather")).await?;

// Choose a custom round cap.
let result = client
    .call_tool_with_mrtr_max_rounds(CallToolRequestParams::new("weather"), 3)
    .await?;

// Manual mode: get the intermediate InputRequiredResult and drive rounds yourself.
match client.call_tool_once(CallToolRequestParams::new("weather")).await? {
    CallToolResponse::InputRequired(input_required) => { /* fulfill + retry */ }
    CallToolResponse::Complete(result) => { /* done */ }
    _ => {}
}
```

**Example:** [`examples/servers/src/mrtr.rs`](examples/servers/src/mrtr.rs) (end-to-end server + client)

---

## Tasks (long-running tool invocations)

`rmcp` implements the [MCP Tasks extension](https://modelcontextprotocol.io/extensions/tasks/overview)
(SEP-2663, `io.modelcontextprotocol/tasks`). A client declares the extension in its
capabilities; the server then decides per request whether to materialize a `tools/call`
as a task, returning a `CreateTaskResult` (`resultType: "task"`). The client polls
`tasks/get`, answers in-task input requests via `tasks/update`, and may request
cooperative cancellation via `tasks/cancel`. Use `rmcp::task_manager::TaskManager`
to manage task lifecycles server-side.

```rust, ignore
// Client: declare the tasks extension capability.
let caps = ClientCapabilities::builder().enable_tasks().build();

// Server: decide per request whether to materialize a task.
async fn call_tool(&self, request: CallToolRequestParams, context: RequestContext<RoleServer>)
    -> Result<CallToolResponse, McpError>
{
    let client_supports_tasks = context
        .client_capabilities()
        .is_some_and(|caps| caps.supports_tasks());
    if client_supports_tasks {
        let task = self.tasks.spawn(TaskOptions::default(), move |_ctx| {
            Box::pin(async move { /* long-running work -> Ok(CallToolResult) */ })
        });
        return Ok(CallToolResponse::Task(CreateTaskResult::new(task)));
    }
    // ... fall back to synchronous execution
}
```

See [`servers_task_stdio`](examples/servers/src/task_stdio.rs) and the matching
[`clients_task_stdio`](examples/clients/src/task_stdio.rs) for a runnable end-to-end example.

## Caching

`rmcp` clients transparently cache responses that carry the
[SEP-2549](https://modelcontextprotocol.io/specification/2026-07-28/server/utilities/caching)
caching hints (`ttlMs` / `cacheScope`) for `server/discover`, `tools/list`,
`prompts/list`, `resources/list`, `resources/templates/list`, and `resources/read`.

Caching is on by default but only stores a response when the server sends a
positive `ttlMs`, so servers that omit the hint behave exactly as before. Entries
expire after their TTL, are partitioned by cache scope, and are invalidated
automatically by the matching `list_changed` / `resource updated` notifications.

No call-site changes are needed — existing calls benefit automatically:

```rust, ignore
let tools = peer.list_tools(None).await?;     // served from cache while fresh
let res   = peer.read_resource(params).await?; // cached per-URI
```

Tune or disable it per connection via the `Peer`:

```rust, ignore
use std::time::Duration;
use rmcp::ClientCacheConfig;

// Customize behavior.
peer.set_response_cache_config(
    ClientCacheConfig::default()
        .with_default_ttl(Duration::from_secs(30)) // TTL for servers that omit ttlMs
        .with_max_ttl(Duration::from_secs(3600))   // upper bound on any TTL
        .with_max_entries(1024)
        .with_private_partition(user_id)            // separate private caches per principal
        .with_serve_stale_on_error(false),          // surface errors instead of stale data
).await;

// Or turn it off entirely.
peer.set_response_cache_config(ClientCacheConfig::disabled()).await;

// Manually flush.
peer.clear_response_cache().await;
```

> **Note:** with the default `serve_stale_on_error`, a failed re-fetch returns the
> last cached response (even if expired) as `Ok(..)` instead of an error. Set
> `with_serve_stale_on_error(false)` if callers must observe fetch failures.

## Standard HTTP Headers

Protocol `2026-07-28` standardizes a set of Streamable HTTP request headers
(SEP-2243) so proxies and gateways can route MCP traffic without parsing the
JSON body: `Mcp-Method`, `Mcp-Name`, and `Mcp-Param-*`. `rmcp` emits and
validates these automatically once a connection negotiates `2026-07-28` or
newer — no call-site changes are required, and older negotiated versions are
untouched.

**MCP Spec:** [Header standardization](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports#header)

- `Mcp-Method` — the JSON-RPC method (e.g. `tools/call`).
- `Mcp-Name` — the target name, sourced from `params.name` (`tools/call`,
  `prompts/get`), `params.uri` (`resources/*`), or `params.taskId` (`tasks/*`).
- `Mcp-Param-*` — selected `tools/call` arguments, promoted from the tool's
  input schema.

To promote a tool argument into a routing header, annotate the top-level schema
property with `x-mcp-header`:

```rust, ignore
// A `region` argument surfaces as the `Mcp-Param-Region` request header.
let schema = serde_json::json!({
    "type": "object",
    "properties": {
        "region": { "type": "string", "x-mcp-header": "Region" }
    }
});
```

Annotations must be non-empty RFC 9110 tokens, case-insensitively unique, and
applied only to top-level primitive (`string`/`integer`/`boolean`) properties.
Values that cannot travel as a bare header (leading/trailing whitespace,
control/non-ASCII characters) are transparently Base64-wrapped as
`=?base64?<b64>?=`.

---

## Stateless Streamable HTTP

Per SEP-2567, `rmcp` serves the `2026-07-28` draft statelessly **automatically**:
no `Mcp-Session-Id`, no standalone GET/DELETE stream, and no `Last-Event-ID`
resumption. The `legacy_session_mode` flag below only controls behavior for
*legacy* protocol versions (`< 2026-07-28`).

**MCP Spec:** [Transports](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports)

### Server-side

A default server already serves `2026-07-28` clients statelessly. To also serve
*legacy* clients without sessions, disable `legacy_session_mode` (formerly
`stateful_mode`; builder `with_stateful_mode`). Optionally set
`with_json_response(true)` so simple request/response tools reply with a single
`application/json` body instead of an SSE stream (the server still falls back to
`text/event-stream` if a handler emits a notification or server request first):

```rust, ignore
use rmcp::transport::streamable_http_server::{
    StreamableHttpService, StreamableHttpServerConfig,
    session::local::LocalSessionManager,
};

let config = StreamableHttpServerConfig::default()
    .with_legacy_session_mode(false) // stateless for legacy versions too
    .with_json_response(true);       // plain JSON replies for simple tools

let service = StreamableHttpService::new(
    || Ok(Counter::new()),           // a fresh handler per request
    LocalSessionManager::default().into(),
    config,
);

// `StreamableHttpService` is a Tower service — mount it on any router.
let router = axum::Router::new().nest_service("/mcp", service);
```

> Because there is no per-session state, the `service_factory` runs per request.
> Keep shared state (DB pools, caches) in a `Clone` handle captured by the
> closure; don't rely on in-memory state surviving between requests.
>
> Modern-only servers can additionally call
> `with_stateless_protocol_metadata_required(true)` to reject the compatibility
> fallback for requests missing their per-request protocol signals. rmcp clients
> negotiated below `2026-07-28` do not attach that body metadata and will be
> rejected, so pair this option with a `supported_protocol_versions`
> implementation that advertises only `2026-07-28` and later.

### Client-side

The Streamable HTTP client transport allows stateless operation by default
(`allow_stateless: true`), so no configuration is needed to talk to a stateless
server — it simply omits the session header when the server doesn't issue one:

```rust, ignore
use rmcp::transport::StreamableHttpClientTransport;

// Defaults are stateless-friendly.
let transport = StreamableHttpClientTransport::from_uri("http://localhost:8000/mcp");
let client = ClientInfo::default().serve(transport).await?;
```

**Example:** [`examples/servers/src/counter_streamhttp.rs`](examples/servers/src/counter_streamhttp.rs) (server), [`examples/clients/src/streamable_http.rs`](examples/clients/src/streamable_http.rs) (client)

---

## Transports

A transport moves JSON-RPC messages between client and server. Any `Transport`
impl can be passed to `.serve(..)`; `rmcp` ships the common ones behind Cargo
features.

**MCP Spec:** [Transports](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports)

| Transport                    | Feature(s)                                   | Notes |
| ---------------------------- | -------------------------------------------- | ----- |
| **stdio**                    | `transport-io` (client + server)             | Communicate over `stdin`/`stdout`; the standard way to launch local MCP servers as child processes. |
| **Child process** (client)   | `transport-child-process`                    | Spawn a server binary and talk to it over its stdio. |
| **Streamable HTTP** (server) | `transport-streamable-http-server`           | The current HTTP transport. Exposes a Tower service you can mount on any router. |
| **Streamable HTTP** (client) | `transport-streamable-http-client-reqwest`   | HTTP client transport built on `reqwest`. |
| **Worker / in-process**      | `transport-worker`                           | For embedding or testing without real I/O. |

### stdio

```rust,ignore
use rmcp::{ServiceExt, transport::stdio};

// Server: serve over stdin/stdout.
let server = MyServer.serve(stdio()).await?;
server.waiting().await?;
```

```rust,ignore
use rmcp::{ServiceExt, transport::{TokioChildProcess, ConfigureCommandExt}};
use tokio::process::Command;

// Client: launch a server binary and talk to it over its stdio.
let transport = TokioChildProcess::new(Command::new("uvx").configure(|cmd| {
    cmd.arg("mcp-server-git");
}))?;
let client = ().serve(transport).await?;
```

### Streamable HTTP

`StreamableHttpService` is a Tower service — mount it on any `axum`/`hyper`
router (see [Stateless Streamable HTTP](#stateless-streamable-http) for the full
server example). The client transport connects with a single URI:

```rust,ignore
use rmcp::transport::StreamableHttpClientTransport;

let transport = StreamableHttpClientTransport::from_uri("http://localhost:8000/mcp");
let client = ClientInfo::default().serve(transport).await?;
```

#### Server-Sent Events (SSE)

Streamable HTTP responses arrive as either a single `application/json` body or a
`text/event-stream` (Server-Sent Events) stream when the server pushes
notifications or requests before the result. `rmcp` handles both automatically
(SSE parsing lives behind the `client-side-sse` feature). There is no separate
"SSE transport" to configure — it's an implementation detail of Streamable HTTP.

#### Legacy HTTP+SSE transport (`2024-11-05`) — intentionally not provided

The standalone two-endpoint **HTTP+SSE transport** defined in protocol revision
`2024-11-05` (a separate `GET` SSE channel plus a `POST` message endpoint) is a
**deliberate non-goal** for `rmcp`. It was [replaced by Streamable HTTP in the
`2025-03-26` revision](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports),
and `rmcp` targets current spec revisions (`2025-11-25` and `2026-07-28`), so it
ships **no legacy HTTP+SSE client or server transport**.

What to use instead:

- **New client/server code** — use [Streamable HTTP](#streamable-http). It carries
  the same SSE streaming semantics over a single endpoint and is the transport all
  supported spec revisions expect.
- **Server-to-client streaming** (push notifications, resource updates) — this is
  built into Streamable HTTP; see [Subscriptions](#subscriptions).
- **Talking to a legacy `2024-11-05`-only server** — front it with a proxy that
  speaks Streamable HTTP, or pin a dependency to a release that predates the
  transport's removal. `rmcp` will not add the legacy transport back.

This is a supported-surface decision, not a missing feature: every transport
`rmcp` implements is listed in the [Transports](#transports) table above.

---

## Pagination

List operations (`tools/list`, `prompts/list`, `resources/list`,
`resources/templates/list`) are paginated via a `next_cursor`. The `list_all_*`
helpers walk every page for you:

```rust,ignore
// Fetches all pages transparently.
let tools     = client.list_all_tools().await?;
let prompts   = client.list_all_prompts().await?;
let resources = client.list_all_resources().await?;
```

To page manually, call the single-page method and follow `next_cursor` until
it's `None`:

```rust,ignore
use rmcp::model::PaginatedRequestParams;

let mut cursor = None;
loop {
    let page = client
        .list_tools(Some(PaginatedRequestParams { meta: None, cursor }))
        .await?;
    for tool in &page.tools {
        // handle each tool
    }
    cursor = page.next_cursor;
    if cursor.is_none() {
        break;
    }
}
```

On the server, return a `next_cursor` from your `list_*` handler when more pages
remain (`None` when complete).

**MCP Spec:** [Pagination](https://modelcontextprotocol.io/specification/2026-07-28/server/utilities/pagination)

---

## Capability & Protocol Version Negotiation

### Capabilities

At initialization, client and server exchange **capabilities** so each side
knows what the other supports. Declare yours with the `ServerCapabilities`
builder in `get_info()`:

```rust,ignore
use rmcp::model::{ServerCapabilities, ServerInfo};

fn get_info(&self) -> ServerInfo {
    ServerInfo::new(
        ServerCapabilities::builder()
            .enable_tools()
            .enable_prompts()
            .enable_resources()
            .enable_resources_subscribe()
            .enable_tool_list_changed()
            .enable_logging()
            .build(),
    )
}
```

Clients do the same via `ClientCapabilities::builder()`. Macros like
`#[tool_handler]` / `#[prompt_handler]` set the relevant flags automatically.
After connecting, read the peer's capabilities via `peer.peer_info()`.

### Protocol version

MCP is versioned by date. `rmcp` negotiates automatically on connect — the
client offers a preferred `ProtocolVersion` and falls back to one the server
supports:

```rust,ignore
use rmcp::model::ProtocolVersion;

ProtocolVersion::LATEST;        // newest stable version this SDK defaults to
ProtocolVersion::V_2026_07_28;  // a specific version constant
ProtocolVersion::KNOWN_VERSIONS; // every version this SDK understands
```

Version-specific behavior (SEP-2243 headers, SEP-2567 stateless serving, the
SEP-2575 subscription model) is gated on the negotiated version, so older clients
keep working while newer ones opt in.

**MCP Spec:** [Versioning and Compatibility](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning)

---

## JSON Schema 2020-12

Deriving `schemars::JsonSchema` on your parameter and result types generates
[JSON Schema draft 2020-12](https://json-schema.org/) — the dialect the MCP spec
requires — for the tool's `inputSchema` and `outputSchema`.

```rust,ignore
use rmcp::schemars;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SearchParams {
    /// Full-text query.
    query: String,
    /// Maximum number of results.
    #[serde(default)]
    limit: Option<u32>,
}
```

Field names, types, and doc comments flow into the schema — no manual authoring
needed. As of `2026-07-28` (SEP-2106), `outputSchema` may be any JSON Schema type
(not only `object`) and `structuredContent` may be any JSON value.

---

## Examples

See [examples](examples/README.md).

## OAuth Support

See [Oauth_support](docs/OAUTH_SUPPORT.md) for details.

## Related Resources

- [MCP Specification](https://modelcontextprotocol.io/specification/2026-07-28)
- [Schema](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/schema/2026-07-28/schema.ts)

## Related Projects

### Extending `rmcp`

- [rmcp-actix-web](https://gitlab.com/lx-industries/rmcp-actix-web) - An `actix_web` backend for `rmcp`
- [rmcp-openapi](https://gitlab.com/lx-industries/rmcp-openapi) - Transform OpenAPI definition endpoints into MCP tools

### Built with `rmcp`

- [goose](https://github.com/block/goose) - An open-source, extensible AI agent that goes beyond code suggestions
- [apollo-mcp-server](https://github.com/apollographql/apollo-mcp-server) - MCP server that connects AI agents to GraphQL APIs via Apollo GraphOS
- [rustfs-mcp](https://github.com/rustfs/rustfs/tree/main/crates/mcp) - High-performance MCP server providing S3-compatible object storage operations for AI/LLM integration
- [containerd-mcp-server](https://github.com/jokemanfire/mcp-containerd) - A containerd-based MCP server implementation
- [rmcp-openapi-server](https://gitlab.com/lx-industries/rmcp-openapi/-/tree/main/crates/rmcp-openapi-server) - High-performance MCP server that exposes OpenAPI definition endpoints as MCP tools
- [nvim-mcp](https://github.com/linw1995/nvim-mcp) - A MCP server to interact with Neovim
- [terminator](https://github.com/mediar-ai/terminator) - AI-powered desktop automation MCP server with cross-platform support and >95% success rate
- [stakpak-agent](https://github.com/stakpak/agent) - Security-hardened terminal agent for DevOps with MCP over mTLS, streaming, secret tokenization, and async task management
- [video-transcriber-mcp-rs](https://github.com/nhatvu148/video-transcriber-mcp-rs) - High-performance MCP server for transcribing videos from 1000+ platforms using whisper.cpp
- [NexusCore MCP](https://github.com/sjkim1127/Nexuscore_MCP) - Advanced malware analysis & dynamic instrumentation MCP server with Frida integration and stealth unpacking capabilities
- [spreadsheet-mcp](https://github.com/PSU3D0/spreadsheet-mcp) - Token-efficient MCP server for spreadsheet analysis with automatic region detection, recalculation, screenshot, and editing support for LLM agents
- [hyper-mcp](https://github.com/hyper-mcp-rs/hyper-mcp) - A fast, secure MCP server that extends its capabilities through WebAssembly (WASM) plugins
- [rudof-mcp](https://github.com/rudof-project/rudof/tree/master/rudof_mcp) - RDF validation and data processing MCP server with ShEx/SHACL validation, SPARQL queries, and format conversion. Supports stdio and streamable HTTP transports with full MCP capabilities (tools, prompts, resources, logging, completions, tasks)
- [MCPMate](https://github.com/loocor/MCPMate) - Desktop app for progressive MCP management: start with guided server import, then grow into multi-client profiles and Unify meta tools to keep tool exposure, token use, and runtime state under control, with more options for efficiency, cost, and reliability
- [McpMux](https://github.com/mcpmux/mcp-mux) - Desktop app to configure MCP servers once at McpMux, connect every AI client (Cursor, Claude Desktop, VS Code, Windsurf) through a single encrypted local gateway with Spaces for project organization, FeatureSets to switch toolsets per client, and a built-in server registry
- [systemprompt-template](https://github.com/systempromptio/systemprompt-template) - Single-binary Rust runtime providing MCP governance — authentication, authorisation, rate-limiting, audit trails, and cost tracking for AI agents. Self-hosted, air-gap capable, 3,300+ req/s with sub-5ms governance overhead
- [jilebi-mcp](https://github.com/datron/jilebi) - an extensible MCP server through plugins in Javascript with a secure permissions model

## Development

### Tips for Contributors

See [docs/CONTRIBUTE.MD](docs/CONTRIBUTE.MD) to get some tips for contributing.

### Using Dev Container

If you want to use dev container, see [docs/DEVCONTAINER.md](docs/DEVCONTAINER.md) for instructions on using Dev Container for development.
