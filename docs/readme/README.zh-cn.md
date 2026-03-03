<div align = "right">
<a href="../../README.md">English</a>
</div>

# RMCP
[![Crates.io Version](https://img.shields.io/crates/v/rmcp)](https://crates.io/crates/rmcp)
[![docs.rs](https://img.shields.io/docsrs/rmcp)](https://docs.rs/rmcp/latest/rmcp)
[![CI](https://github.com/modelcontextprotocol/rust-sdk/actions/workflows/ci.yml/badge.svg)](https://github.com/modelcontextprotocol/rust-sdk/actions/workflows/ci.yml)
[![License](https://img.shields.io/crates/l/rmcp)](../../LICENSE)

一个基于 tokio 异步运行时的官方 Rust Model Context Protocol SDK 实现。

本仓库包含以下 crate：

- [rmcp](../../crates/rmcp)：实现 RMCP 协议的核心库 - 详见 [rmcp](../../crates/rmcp/README.md)
- [rmcp-macros](../../crates/rmcp-macros)：用于生成 RMCP 工具实现的过程宏库 - 详见 [rmcp-macros](../../crates/rmcp-macros/README.md)

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
let server = service.serve(transport).await?;
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


## 示例

查看 [examples](../../examples/README.md)。

## 功能文档

查看 [docs/FEATURES.md](../FEATURES.md) 了解核心 MCP 功能的详细文档：资源、提示词、采样、根目录、日志、补全、通知和订阅。

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
