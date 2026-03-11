<div align = "right">
<a href="../../README.md">English</a>
</div>

# RMCP
[![Crates.io Version](https://img.shields.io/crates/v/rmcp)](https://crates.io/crates/rmcp)
[![docs.rs](https://img.shields.io/docsrs/rmcp)](https://docs.rs/rmcp/latest/rmcp)
[![CI](https://github.com/modelcontextprotocol/rust-sdk/actions/workflows/ci.yml/badge.svg)](https://github.com/modelcontextprotocol/rust-sdk/actions/workflows/ci.yml)
[![License](https://img.shields.io/crates/l/rmcp)](../../LICENSE)

一个基于 tokio 异步运行时的官方 Rust Model Context Protocol SDK 实现。

> **迁移到 1.x？** 请参阅 [迁移指南](https://github.com/modelcontextprotocol/rust-sdk/discussions/716) 了解破坏性变更和升级说明。

本仓库包含以下 crate：

- [rmcp](../../crates/rmcp)：实现 RMCP 协议的核心库 - 详见 [rmcp](../../crates/rmcp/README.md)
- [rmcp-macros](../../crates/rmcp-macros)：用于生成 RMCP 工具实现的过程宏库 - 详见 [rmcp-macros](../../crates/rmcp-macros/README.md)

完整的 MCP 规范请参阅 [modelcontextprotocol.io](https://modelcontextprotocol.io/specification/2025-11-25)。

## 目录

- [使用](#使用)
- [资源](#资源)
- [提示词](#提示词)
- [采样](#采样)
- [根目录](#根目录)
- [日志](#日志)
- [补全](#补全)
- [通知](#通知)
- [订阅](#订阅)
- [示例](#示例)
- [OAuth 支持](#oauth-支持)
- [相关资源](#相关资源)
- [相关项目](#相关项目)
- [开发](#开发)

## 使用

### 导入

```toml
rmcp = { version = "0.16.0", features = ["server"] }
## 或使用最新开发版本
rmcp = { git = "https://github.com/modelcontextprotocol/rust-sdk", branch = "main" }
```
### 第三方依赖

基本依赖：
- [tokio](https://github.com/tokio-rs/tokio)
- [serde](https://github.com/serde-rs/serde)
JSON Schema 生成 (version 2020-12)：
- [schemars](https://github.com/GREsau/schemars)

### 构建客户端

<details>
<summary>启动客户端</summary>

```rust, ignore
use rmcp::transport::{
    CommandBuilder,
    ChildProcessTransport,
    tokio::TokioChildProcessRunner 
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let command = CommandBuilder::<TokioChildProcessRunner>::new("npx")
        .arg("-y")
        .arg("@modelcontextprotocol/server-everything")
        .spawn_dyn()?

    let transport = ChildProcessTransport::new(command)?

    let (client, work) = ().serve(transport).await?;
    tokio::spawn(work);

    // ...

    client.canel().await;
    Ok(())
}
```
</details>

### 构建服务端

<details>
<summary>构建传输层</summary>

```rust, ignore
use tokio::io::{stdin, stdout};
let transport = (stdin(), stdout());
```

</details>

<details>
<summary>构建服务</summary>

你可以通过 [`ServerHandler`](../../crates/rmcp/src/handler/server.rs) 或 [`ClientHandler`](../../crates/rmcp/src/handler/client.rs) 轻松构建服务。

```rust, ignore
let service = common::counter::Counter::new();
```
</details>

<details>
<summary>启动服务端</summary>

```rust, ignore
// 此调用将完成初始化过程
let (server, work) = service.serve(transport).await?;
tokio::spawn(work);
```
</details>

<details>
<summary>与服务端交互</summary>

服务端初始化完成后，你可以发送请求或通知：

```rust, ignore
// 请求
let roots = server.list_roots().await?;

// 或发送通知
server.notify_cancelled(...).await?;
```
</details>

<details>
<summary>等待服务停止</summary>

```rust, ignore
let quit_reason = server.waiting().await?;
// 或将其取消
let quit_reason = server.cancel().await?;
```
</details>

---

## 资源

资源允许服务端向客户端暴露数据（文件、数据库记录、API 响应）供其读取。每个资源通过 URI 标识，返回文本或二进制（base64 编码）内容。资源模板允许服务端声明带有动态参数的 URI 模式。

**MCP 规范：** [Resources](https://modelcontextprotocol.io/specification/2025-11-25/server/resources)

### 服务端

在 `ServerHandler` trait 上实现 `list_resources()`、`read_resource()`，以及可选的 `list_resource_templates()`。在 `get_info()` 中启用资源能力。

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

### 客户端

```rust
use rmcp::model::{ReadResourceRequestParams};

// 列出所有资源（自动处理分页）
let resources = client.list_all_resources().await?;

// 通过 URI 读取特定资源
let result = client.read_resource(ReadResourceRequestParams {
    meta: None,
    uri: "file:///config.json".into(),
}).await?;

// 列出资源模板
let templates = client.list_all_resource_templates().await?;
```

### 通知

服务端可以在资源列表变更或特定资源更新时通知客户端：

```rust
// 通知资源列表已变更（客户端应重新获取）
context.peer.notify_resource_list_changed().await?;

// 通知特定资源已更新
context.peer.notify_resource_updated(ResourceUpdatedNotificationParam {
    uri: "file:///config.json".into(),
}).await?;
```

客户端通过 `ClientHandler` 处理这些通知：

```rust
impl ClientHandler for MyClient {
    async fn on_resource_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) {
        // 重新获取资源列表
    }

    async fn on_resource_updated(
        &self,
        params: ResourceUpdatedNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        // 重新读取 params.uri 对应的资源
    }
}
```

**示例：** [`examples/servers/src/common/counter.rs`](../../examples/servers/src/common/counter.rs)（服务端），[`examples/clients/src/everything_stdio.rs`](../../examples/clients/src/everything_stdio.rs)（客户端）

---

## 提示词

提示词是服务端向客户端暴露的可复用消息模板。它们接受类型化参数并返回对话消息。`#[prompt]` 宏自动处理参数验证和路由。

**MCP 规范：** [Prompts](https://modelcontextprotocol.io/specification/2025-11-25/server/prompts)

### 服务端

使用 `#[prompt_router]`、`#[prompt]` 和 `#[prompt_handler]` 宏以声明式方式定义提示词。参数定义为派生 `JsonSchema` 的结构体。

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

    /// 无参数的简单提示词
    #[prompt(name = "greeting", description = "A simple greeting")]
    async fn greeting(&self) -> Vec<PromptMessage> {
        vec![PromptMessage::new_text(
            PromptMessageRole::User,
            "Hello! How can you help me today?",
        )]
    }

    /// 带类型化参数的提示词
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

提示词函数支持以下返回类型：
- `Vec<PromptMessage>` -- 简单消息列表
- `GetPromptResult` -- 带可选描述的消息
- `Result<T, McpError>` -- 以上任一类型，附带错误处理

### 客户端

```rust
use rmcp::model::GetPromptRequestParams;

// 列出所有提示词
let prompts = client.list_all_prompts().await?;

// 带参数获取提示词
let result = client.get_prompt(GetPromptRequestParams {
    meta: None,
    name: "code_review".into(),
    arguments: Some(rmcp::object!({
        "language": "Rust",
        "focus_areas": ["performance", "safety"]
    })),
}).await?;
```

### 通知

```rust
// 服务端：通知可用提示词已变更
context.peer.notify_prompt_list_changed().await?;
```

**示例：** [`examples/servers/src/prompt_stdio.rs`](../../examples/servers/src/prompt_stdio.rs)（服务端），[`examples/clients/src/everything_stdio.rs`](../../examples/clients/src/everything_stdio.rs)（客户端）

---

## 采样

采样反转了通常的方向：服务端请求客户端执行 LLM 补全。服务端发送 `create_message` 请求，客户端通过其 LLM 处理并返回结果。

**MCP 规范：** [Sampling](https://modelcontextprotocol.io/specification/2025-11-25/client/sampling)

### 服务端（请求采样）

通过 `context.peer.create_message()` 访问客户端的采样能力：

```rust
use rmcp::model::*;

// 在 ServerHandler 方法内部（例如 call_tool）：
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

// 提取响应文本
let text = response.message.content
    .first()
    .and_then(|c| c.as_text())
    .map(|t| &t.text);
```

### 客户端（处理采样）

在客户端实现 `ClientHandler::create_message()`。这是你调用实际 LLM 的地方：

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
        // 转发到你的 LLM，或返回模拟响应：
        let response_text = call_your_llm(&params.messages).await;

        Ok(CreateMessageResult {
            message: SamplingMessage::assistant_text(response_text),
            model: "my-model".into(),
            stop_reason: Some(CreateMessageResult::STOP_REASON_END_TURN.into()),
        })
    }
}
```

**示例：** [`examples/servers/src/sampling_stdio.rs`](../../examples/servers/src/sampling_stdio.rs)（服务端），[`examples/clients/src/sampling_stdio.rs`](../../examples/clients/src/sampling_stdio.rs)（客户端）

---

## 根目录

根目录告诉服务端客户端正在使用哪些目录或项目。根目录是一个 URI（通常为 `file://`），指向工作区或代码仓库。服务端可以查询根目录以了解在哪里查找文件以及如何限定工作范围。

**MCP 规范：** [Roots](https://modelcontextprotocol.io/specification/2025-11-25/client/roots)

### 服务端

向客户端请求根目录列表，并处理变更通知：

```rust
use rmcp::{ServerHandler, model::*, service::{NotificationContext, RoleServer}};

impl ServerHandler for MyServer {
    // 向客户端查询根目录
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let roots = context.peer.list_roots().await?;
        // 使用 roots.roots 了解工作区边界
        // ...
    }

    // 当客户端的根目录列表变更时调用
    async fn on_roots_list_changed(
        &self,
        _context: NotificationContext<RoleServer>,
    ) {
        // 重新获取根目录以保持最新
    }
}
```

### 客户端

客户端声明根目录能力并实现 `list_roots()`：

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

客户端在根目录变更时通知服务端：

```rust
// 添加或移除工作区根目录后：
client.notify_roots_list_changed().await?;
```

---

## 日志

服务端可以向客户端发送结构化日志消息。客户端设置最低严重级别，服务端通过对等通知接口发送消息。

**MCP 规范：** [Logging](https://modelcontextprotocol.io/specification/2025-11-25/server/utilities/logging)

### 服务端

启用日志能力，处理客户端的级别变更，并通过对等端发送日志消息：

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

    // 客户端设置最低日志级别
    async fn set_level(
        &self,
        request: SetLevelRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        // 存储 request.level 并据此过滤后续日志消息
        Ok(())
    }
}

// 在任何可以访问 peer 的处理器中发送日志消息：
context.peer.notify_logging_message(LoggingMessageNotificationParam {
    level: LoggingLevel::Info,
    logger: Some("my-server".into()),
    data: serde_json::json!({
        "message": "Processing completed",
        "items_processed": 42
    }),
}).await?;
```

可用日志级别（从低到高）：`Debug`、`Info`、`Notice`、`Warning`、`Error`、`Critical`、`Alert`、`Emergency`。

### 客户端

客户端通过 `ClientHandler` 处理传入的日志消息：

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

客户端也可以设置服务端的日志级别：

```rust
client.set_level(SetLevelRequestParams {
    level: LoggingLevel::Warning,
    meta: None,
}).await?;
```

---

## 补全

补全为提示词或资源模板参数提供自动补全建议。当用户填写参数时，客户端可以根据已输入的内容向服务端请求建议。

**MCP 规范：** [Completions](https://modelcontextprotocol.io/specification/2025-11-25/server/utilities/completion)

### 服务端

启用补全能力并实现 `complete()` 处理器。使用 `request.context` 检查已填写的参数：

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
                        // 根据已填写的参数调整建议
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

        // 根据用户的部分输入进行过滤
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

### 客户端

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

// result.completion.values 包含建议，例如 ["SELECT"]
```

**示例：** [`examples/servers/src/completion_stdio.rs`](../../examples/servers/src/completion_stdio.rs)

---

## 通知

通知是即发即忘的消息——不需要响应。它们涵盖进度更新、取消和生命周期事件。双方都可以发送和接收通知。

**MCP 规范：** [Notifications](https://modelcontextprotocol.io/specification/2025-11-25/basic/notifications)

### 进度通知

服务端可以在长时间运行的操作中报告进度：

```rust
use rmcp::model::*;

// 在工具处理器内部：
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

### 取消

任一方都可以取消正在进行的请求：

```rust
// 发送取消通知
context.peer.notify_cancelled(CancelledNotificationParam {
    request_id: the_request_id,
    reason: Some("User requested cancellation".into()),
}).await?;
```

在 `ServerHandler` 或 `ClientHandler` 中处理取消：

```rust
impl ServerHandler for MyServer {
    async fn on_cancelled(
        &self,
        params: CancelledNotificationParam,
        _context: NotificationContext<RoleServer>,
    ) {
        // 中止 params.request_id 对应的工作
    }
}
```

### 初始化通知

客户端在握手完成后发送 `initialized` 通知：

```rust
// 在 serve() 握手过程中由 rmcp 自动发送。
// 服务端通过以下方式处理：
impl ServerHandler for MyServer {
    async fn on_initialized(
        &self,
        _context: NotificationContext<RoleServer>,
    ) {
        // 服务端已准备好接收请求
    }
}
```

### 列表变更通知

当可用的工具、提示词或资源发生变更时，通知客户端：

```rust
context.peer.notify_tool_list_changed().await?;
context.peer.notify_prompt_list_changed().await?;
context.peer.notify_resource_list_changed().await?;
```

**示例：** [`examples/servers/src/common/progress_demo.rs`](../../examples/servers/src/common/progress_demo.rs)

---

## 订阅

客户端可以订阅特定资源。当订阅的资源发生变更时，服务端发送通知，客户端可以重新读取该资源。

**MCP 规范：** [Resources - Subscriptions](https://modelcontextprotocol.io/specification/2025-11-25/server/resources#subscriptions)

### 服务端

在资源能力中启用订阅，并实现 `subscribe()` / `unsubscribe()` 处理器：

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

当订阅的资源发生变更时，通知客户端：

```rust
// 检查资源是否有订阅者，然后通知
context.peer.notify_resource_updated(ResourceUpdatedNotificationParam {
    uri: "file:///config.json".into(),
}).await?;
```

### 客户端

```rust
use rmcp::model::*;

// 订阅资源更新
client.subscribe(SubscribeRequestParams {
    meta: None,
    uri: "file:///config.json".into(),
}).await?;

// 不再需要时取消订阅
client.unsubscribe(UnsubscribeRequestParams {
    meta: None,
    uri: "file:///config.json".into(),
}).await?;
```

在 `ClientHandler` 中处理更新通知：

```rust
impl ClientHandler for MyClient {
    async fn on_resource_updated(
        &self,
        params: ResourceUpdatedNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        // 重新读取 params.uri 对应的资源
    }
}
```

---

## 示例

查看 [examples](../../examples/README.md)。

## OAuth 支持

查看 [OAuth 支持](../OAUTH_SUPPORT.md) 了解详情。

## 相关资源

- [MCP 规范](https://modelcontextprotocol.io/specification/2025-11-25)
- [Schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2025-11-25/schema.ts)

## 相关项目

### 扩展 `rmcp`

- [rmcp-actix-web](https://gitlab.com/lx-industries/rmcp-actix-web) - 基于 `actix_web` 的 `rmcp` 后端
- [rmcp-openapi](https://gitlab.com/lx-industries/rmcp-openapi) - 将 OpenAPI 定义的端点转换为 MCP 工具

### 基于 `rmcp` 构建

- [goose](https://github.com/block/goose) - 一个超越代码建议的开源、可扩展 AI 智能体
- [apollo-mcp-server](https://github.com/apollographql/apollo-mcp-server) - 通过 Apollo GraphOS 将 AI 智能体连接到 GraphQL API 的 MCP 服务
- [rustfs-mcp](https://github.com/rustfs/rustfs/tree/main/crates/mcp) - 为 AI/LLM 集成提供 S3 兼容对象存储操作的高性能 MCP 服务
- [containerd-mcp-server](https://github.com/jokemanfire/mcp-containerd) - 基于 containerd 实现的 MCP 服务
- [rmcp-openapi-server](https://gitlab.com/lx-industries/rmcp-openapi/-/tree/main/crates/rmcp-openapi-server) - 将 OpenAPI 定义的端点暴露为 MCP 工具的高性能 MCP 服务
- [nvim-mcp](https://github.com/linw1995/nvim-mcp) - 与 Neovim 交互的 MCP 服务
- [terminator](https://github.com/mediar-ai/terminator) - AI 驱动的桌面自动化 MCP 服务，支持跨平台，成功率超过 95%
- [stakpak-agent](https://github.com/stakpak/agent) - 安全加固的 DevOps 终端智能体，支持 MCP over mTLS、流式传输、密钥令牌化和异步任务管理
- [video-transcriber-mcp-rs](https://github.com/nhatvu148/video-transcriber-mcp-rs) - 使用 whisper.cpp 从 1000+ 平台转录视频的高性能 MCP 服务
- [NexusCore MCP](https://github.com/sjkim1127/Nexuscore_MCP) - 具有 Frida 集成和隐蔽脱壳功能的高级恶意软件分析与动态检测 MCP 服务
- [spreadsheet-mcp](https://github.com/PSU3D0/spreadsheet-mcp) - 面向 LLM 智能体的高效 Token 使用的电子表格分析 MCP 服务，支持自动区域检测、重新计算、截图和编辑
- [hyper-mcp](https://github.com/hyper-mcp-rs/hyper-mcp) - 通过 WebAssembly (WASM) 插件扩展功能的快速、安全的 MCP 服务
- [rudof-mcp](https://github.com/rudof-project/rudof/tree/master/rudof_mcp) - RDF 验证和数据处理 MCP 服务，支持 ShEx/SHACL 验证、SPARQL 查询和格式转换。支持 stdio 和 Streamable HTTP 传输，具备完整的 MCP 功能（工具、提示词、资源、日志、补全、任务）


## 开发

### 贡献指南

查看 [docs/CONTRIBUTE.MD](../CONTRIBUTE.MD) 获取贡献提示。

### 使用 Dev Container

如果你想使用 Dev Container，查看 [docs/DEVCONTAINER.md](../DEVCONTAINER.md) 获取开发指南。
