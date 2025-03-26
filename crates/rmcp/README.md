# RMCP
[![Crates.io Version](https://img.shields.io/crates/v/rmcp)](https://crates.io/crates/rmcp)
![Release status](https://github.commodelcontextprotocol/rust-sdk/actions/workflows/release.yml/badge.svg)
[![docs.rs](https://img.shields.io/docsrs/rmcp)](https://docs.rs/rmcp/latest/rmcp)

A better and clean rust Model Context Protocol SDK implementation with tokio async runtime.

## Comparing to official SDK

The [Official SDK](https://github.com/modelcontextprotocol/rust-sdk/pulls) has too much limit and it was originally built for [goose](https://github.com/block/goose) rather than general using purpose.

All the features listed on specification would be implemented in this crate. And the first and most important thing is, this crate has the correct and intact data [types](crate::model). See it yourself. 

## Usage

### Import
```toml
rmcp = { version = "0.1", features = ["server"] }
```

### Quick start
Start a client in one line:
```rust,ignore
# use rmcp::{ServiceExt, transport::child_process::TokioChildProcess};
# use tokio::process::Command;

let client = ().serve(
    TokioChildProcess::new(Command::new("npx").arg("-y").arg("@modelcontextprotocol/server-everything"))?
).await?;
```


Start a client in one line:
```rust,ignore
# use rmcp::{ServiceExt, transport::TokioChildProcess};
# use tokio::process::Command;

let client = ().serve(
    TokioChildProcess::new(Command::new("npx").arg("-y").arg("@modelcontextprotocol/server-everything"))?
).await?;
```


#### 1. Build a transport
The transport type must implemented [`IntoTransport`](crate::transport::IntoTransport) trait, which allow split into a sink and a stream.

For client, the sink item is [`ClientJsonRpcMessage`](crate::model::ClientJsonRpcMessage) and stream item is [`ServerJsonRpcMessage`](crate::model::ServerJsonRpcMessage)

For server, the sink item is [`ServerJsonRpcMessage`](crate::model::ServerJsonRpcMessage) and stream item is [`ClientJsonRpcMessage`](crate::model::ClientJsonRpcMessage)

##### These types is automatically implemented [`IntoTransport`](crate::transport::IntoTransport) trait
1. For type that already implement both [`Sink`](futures::Sink) and [`Stream`](futures::Stream) trait, they are automatically implemented [`IntoTransport`](crate::transport::IntoTransport) trait
2. For tuple of sink `Tx` and stream `Rx`, type `(Tx, Rx)` are automatically implemented [`IntoTransport`](crate::transport::IntoTransport) trait
3. For type that implement both [`tokio::io::AsyncRead`] and [`tokio::io::AsyncWrite`] trait, they are automatically implemented [`IntoTransport`](crate::transport::IntoTransport) trait
4. For tuple of [`tokio::io::AsyncRead`] `R `and [`tokio::io::AsyncWrite`] `W`, type `(R, W)` are automatically implemented [`IntoTransport`](crate::transport::IntoTransport) trait


```rust, ignore
use tokio::io::{stdin, stdout};
let transport = (stdin(), stdout());
```

#### 2. Build a service
You can easily build a service by using [`ServerHandler`](crate::handler::server) or [`ClientHandler`](crate::handler::client).

```rust, ignore
let service = common::counter::Counter::new();
```

Or if you want to use `tower`, you can [`TowerHandler`] as a adapter.

You can reference the [server examples](https://github.commodelcontextprotocol/rust-sdk/tree/release/examples/servers).

#### 3. Serve them together
```rust, ignore
// this call will finish the initialization process
let server = service.serve(transport).await?;
```

#### 4. Interact with the server
Once the server is initialized, you can send requests or notifications:

```rust, ignore
// request 
let roots = server.list_roots().await?;

// or send notification
server.notify_cancelled(...).await?;
```

#### 5. Waiting for service shutdown
```rust, ignore
let quit_reason = server.waiting().await?;
// or cancel it
let quit_reason = server.cancel().await?;
```

### Use marcos to declaring tool
Use `toolbox` and `tool` macros to create tool quickly.

Check this [file](https://github.commodelcontextprotocol/rust-sdk/tree/release/examples/servers/src/common/calculator.rs).
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
    #[tool(description = "Calculate the sum of two numbers")]
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

### Manage Multi Services
For many cases you need to manage several service in a collection, you can call `into_dyn` to convert services into the same type.
```rust, ignore
let service = service.into_dyn();
```


### Examples
See [examples](https://github.commodelcontextprotocol/rust-sdk/tree/release/examples/README.md)

### Features
- `client`: use client side sdk
- `server`: use server side sdk
- `macros`: macros default
#### Transports
- `transport-io`: Server stdio transport
- `transport-sse-server`: Server SSE transport
- `transport-child-process`: Client stdio transport
- `transport-sse`: Client sse transport

## Related Resources
- [MCP Specification](https://spec.modelcontextprotocol.io/specification/2024-11-05/)

- [Schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.ts)
