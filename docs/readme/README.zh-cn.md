# RMCP
[![Crates.io Version](https://img.shields.io/crates/v/rmcp)](https://crates.io/crates/rmcp)
![Release status](https://github.commodelcontextprotocol/rust-sdk/actions/workflows/release.yml/badge.svg)
[![docs.rs](https://img.shields.io/docsrs/rmcp)](https://docs.rs/rmcp/latest/rmcp)

一个基于 tokio 异步运行时的官方 Model Context Protocol SDK 实现。

本项目使用了以下开源库:

- [rmcp](crates/rmcp): 实现 RMCP 协议的核心库 (详见：[rmcp](crates/rmcp/README.md))
- [rmcp-macros](crates/rmcp-macros): 一个用于生成 RMCP 工具实现的过程宏库。 (详见：[rmcp-macros](crates/rmcp-macros/README.md))

## 使用

### 导入
```toml
rmcp = { version = "0.2.0", features = ["server"] }
## 或使用最新开发版本
rmcp = { git = "https://github.com/modelcontextprotocol/rust-sdk", branch = "main" }
```

### 第三方依赖库
基本依赖:
- [tokio required](https://github.com/tokio-rs/tokio)
- [serde required](https://github.com/serde-rs/serde)

### 构建客户端
<details>
<summary>构建客户端</summary>

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

You can easily build a service by using [`ServerHandler`](crates/rmcp/src/handler/server.rs) or [`ClientHandler`](crates/rmcp/src/handler/client.rs).

```rust, ignore
let service = common::counter::Counter::new();
```
</details>

<details>
<summary>启动服务端</summary>

```rust, ignore
// this call will finish the initialization process
let server = service.serve(transport).await?;
```
</details>

<details>
<summary>与服务端交互</summary>

Once the server is initialized, you can send requests or notifications:

```rust, ignore
// request
let roots = server.list_roots().await?;

// or send notification
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

### 示例
查看 [examples](examples/README.md)

## OAuth 支持

查看 [oauth_support](docs/OAUTH_SUPPORT.md)

## 相关资源

- [MCP Specification](https://spec.modelcontextprotocol.io/specification/2024-11-05/)
- [Schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.ts)

## 相关项目
- [containerd-mcp-server](https://github.com/jokemanfire/mcp-containerd) - 基于 containerd 实现的 MCP 服务

## 开发

### 贡献指南

查看 [docs/CONTRIBUTE.MD](docs/CONTRIBUTE.MD)

### 使用 Dev Container

如果你想使用 Dev Container，查看 [docs/DEVCONTAINER.md](docs/DEVCONTAINER.md) 获取开发指南。
