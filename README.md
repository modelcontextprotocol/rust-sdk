<div align = "right">
<a href="docs/readme/README.zh-cn.md">简体中文</a>
</div>

# RMCP

[![Crates.io Version](https://img.shields.io/crates/v/rmcp)](https://crates.io/crates/rmcp)
![Release status](https://github.com/modelcontextprotocol/rust-sdk/actions/workflows/release.yml/badge.svg)
[![docs.rs](https://img.shields.io/docsrs/rmcp)](https://docs.rs/rmcp/latest/rmcp)

An official rust Model Context Protocol SDK implementation with tokio async runtime.

## Usage

### Import

```toml
rmcp = { version = "0.1", features = ["server"] }
## or dev channel
rmcp = { git = "https://github.com/modelcontextprotocol/rust-sdk", branch = "main" }
```

### Quick start

Start a client:

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

<details>
<summary>1. Build a transport</summary>

```rust, ignore
use tokio::io::{stdin, stdout};
let transport = (stdin(), stdout());
```

#### Transport
The transport type must implemented [`Transport`] trait, which allow it send message concurrently and receive message sequentially.
There are 3 pairs of standard transport types:

| transport         | client                                                    | server                                                |
|:-:                |:-:                                                        |:-:                                                    |
| std IO            | [`child_process::TokioChildProcess`]                      | [`io::stdio`]                                         |
| streamable http   | [`streamable_http_client::StreamableHttpClientTransport`] | [`streamable_http_server::session::create_session`]   |
| sse               | [`sse_client::SseClientTransport`]                        | [`sse_server::SseServer`]                             |

#### [IntoTransport](`IntoTransport`) trait
[`IntoTransport`] is a helper trait that implicitly convert a type into a transport type.

These types is automatically implemented [`IntoTransport`] trait
1. A type that already implement both [`futures::Sink`] and [`futures::Stream`] trait, or a tuple `(Tx, Rx)`  where `Tx` is [`futures::Sink`] and `Rx` is [`futures::Stream`].
2. A type that implement both [`tokio::io::AsyncRead`] and [`tokio::io::AsyncWrite`] trait. or a tuple `(R, W)` where `R` is [`tokio::io::AsyncRead`] and `W` is [`tokio::io::AsyncWrite`].
3. A type that implement [Worker](`worker::Worker`) trait.
4. A type that implement [`Transport`] trait.

For example, you can see how we build a transport through TCP stream or http upgrade so easily. [examples](examples/README.md)
</details>

<details>
<summary>2. Build a service</summary>

You can easily build a service by using [`ServerHandler`](crates/rmcp/src/handler/server.rs) or [`ClientHandler`](crates/rmcp/src/handler/client.rs).

```rust, ignore
let service = common::counter::Counter::new();
```
</details>

<details>
<summary>3. Serve them together</summary>

```rust, ignore
// this call will finish the initialization process
let server = service.serve(transport).await?;
```
</details>

<details>
<summary>4. Interact with the server</summary>

Once the server is initialized, you can send requests or notifications:

```rust, ignore
// request
let roots = server.list_roots().await?;

// or send notification
server.notify_cancelled(...).await?;
```
</details>

<details>
<summary>5. Waiting for service shutdown</summary>

```rust, ignore
let quit_reason = server.waiting().await?;
// or cancel it
let quit_reason = server.cancel().await?;
```
</details>

### Use macros to declaring tool

Use `toolbox` and `tool` macros to create tool quickly.

<details>
<summary>Example: Calculator Tool</summary>

Check this [file](examples/servers/src/common/calculator.rs).
```rust, ignore
use rmcp::{ServerHandler, model::ServerInfo, schemars, tool};

use super::counter::Counter;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SumRequest {
    #[schemars(description = "the left hand side number")]
    pub a: i32,
    #[schemars(description = "the right hand side number")]
    pub b: i32,
}
#[derive(Debug, Clone)]
pub struct Calculator;

// create a static toolbox to store the tool attributes
#[tool(tool_box)]
impl Calculator {
    // async function
    #[tool(description = "Calculate the sum of two numbers")]
    async fn sum(&self, #[tool(aggr)] SumRequest { a, b }: SumRequest) -> String {
        (a + b).to_string()
    }

    // sync function
    #[tool(description = "Calculate the difference of two numbers")]
    fn sub(
        &self,
        #[tool(param)]
        // this macro will transfer the schemars and serde's attributes
        #[schemars(description = "the left hand side number")]
        a: i32,
        #[tool(param)]
        #[schemars(description = "the right hand side number")]
        b: i32,
    ) -> String {
        (a - b).to_string()
    }
}

// impl call_tool and list_tool by querying static toolbox
#[tool(tool_box)]
impl ServerHandler for Calculator {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some("A simple calculator".into()),
            ..Default::default()
        }
    }
}
```


The only thing you should do is to make the function's return type implement `IntoCallToolResult`.

And you can just implement `IntoContents`, and the return value will be marked as success automatically.

If you return a type of `Result<T, E>` where `T` and `E` both implemented `IntoContents`, it's also OK.
</details>

### Manage Multi Services

For many cases you need to manage several service in a collection, you can call `into_dyn` to convert services into the same type.
```rust, ignore
let service = service.into_dyn();
```

### OAuth Support

See [docs/OAUTH_SUPPORT.md](docs/OAUTH_SUPPORT.md) for details.

### Examples

See [examples](examples/README.md)

### Features

- `client`: use client side sdk
- `server`: use server side sdk
- `macros`: macros default
- `schemars`: implement `JsonSchema` for all model structs

#### Transports

- `transport-io`: Server stdio transport
- `transport-sse-server`: Server SSE transport
- `transport-child-process`: Client stdio transport
- `transport-sse-client`: Client sse transport
- `transport-streamable-http-server` streamable http server transport
- `transport-streamable-client-server` streamable http server transport


## Related Resources

- [MCP Specification](https://spec.modelcontextprotocol.io/specification/2024-11-05/)
- [Schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.ts)

## Related Projects
- [containerd-mcp-server](https://github.com/jokemanfire/mcp-containerd) - A containerd-based MCP server implementation

## Development with Dev Container
See [docs/DEVCONTAINER.md](docs/DEVCONTAINER.md) for instructions on using Dev Container for development.
