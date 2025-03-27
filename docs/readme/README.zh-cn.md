# RMCP
[![Crates.io Version](https://img.shields.io/crates/v/rmcp)](https://crates.io/crates/rmcp)
![Release status](https://github.commodelcontextprotocol/rust-sdk/actions/workflows/release.yml/badge.svg)
[![docs.rs](https://img.shields.io/docsrs/rmcp)](https://docs.rs/rmcp/latest/rmcp)

一个干净且完整的 MCP SDK

## 使用

### 导入
```toml
rmcp = { version = "0.1", features = ["server"] }
## 或者开发者频道
rmcp = { git = "https://github.com/modelcontextprotocol/rust-sdk", branch = "main" }
```

### 快速上手
你可以用一行代码，启动一个SSE客户端
```rust
use rmcp::{ServiceExt, transport::TokioChildProcess};
use tokio::process::Command;

let client = ().serve(
    TokioChildProcess::new(Command::new("npx").arg("-y").arg("@modelcontextprotocol/server-everything"))?
).await?;
```

#### 1. 构建传输层

```rust, ignore
use tokio::io::{stdin, stdout};
let transport = (stdin(), stdout());
```

传输层类型只需要实现 [`IntoTransport`](crate::transport::IntoTransport) trait, 这个特性允许你创建一个Sink和一个Stream

对于客户端, Sink 的 Item 是 [`ClientJsonRpcMessage`](crate::model::ClientJsonRpcMessage)， Stream 的 Item 是 [`ServerJsonRpcMessage`](crate::model::ServerJsonRpcMessage)

对于服务端, Sink 的 Item 是 [`ServerJsonRpcMessage`](crate::model::ServerJsonRpcMessage)， Stream 的 Item 是 [`ClientJsonRpcMessage`](crate::model::ClientJsonRpcMessage)

##### 这些类型自动实现了 [`IntoTransport`](crate::transport::IntoTransport) trait
1. 兼具 [`Sink`](futures::Sink) 与 [`Stream`](futures::Stream) 的
2. 一对 Sink `Tx` Stream `Rx`, 类型 `(Tx, Rx)` 自动实现 [`IntoTransport`](crate::transport::IntoTransport)
3. 兼具 [`tokio::io::AsyncRead`] 与 [`tokio::io::AsyncWrite`] 的
4. 一对 Sink [`tokio::io::AsyncRead`] `R ` [`tokio::io::AsyncWrite`] `W`, 类型 `(R, W)`自动实现 [`IntoTransport`](crate::transport::IntoTransport)

示例，你可以轻松创建一个TCP流来作为传输层. [examples](examples/README.md)

#### 2. 构建服务
你可以通过 [`ServerHandler`](crates/rmcp/src/handler/server.rs) 或 [`ClientHandler`](crates/rmcp/src/handler/client.rs) 轻松构建服务

```rust, ignore
let service = common::counter::Counter::new();
```

如果你想用 `tower`, 你也可以使用 [`TowerHandler`] 来作为tower服务的适配器.

请参考 [服务用例](examples/servers/src/common/counter.rs).

#### 3. 把他们组装到一起
```rust, ignore
// 这里会自动完成初始化流程
let server = service.serve(transport).await?;
```

#### 4. 与服务端/客户端交互
一旦你完成初始化，你可以发送请求或者发送通知

```rust, ignore
// request 
let roots = server.list_roots().await?;

// or send notification
server.notify_cancelled(...).await?;
```

#### 5. 等待服务结束
```rust, ignore
let quit_reason = server.waiting().await?;
// or cancel it
let quit_reason = server.cancel().await?;
```

### 使用宏来定义工具
使用 `tool` 宏来快速创建工具

请看这个[文件](examples/servers/src/common/calculator.rs).
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
你要做的唯一事情就是保证函数的返回类型实现了 `IntoCallToolResult`.

你可以为返回类型实现 `IntoContents`, 那么返回内容会被自动标记为成功。

如果返回类型是  `Result<T, E>` ，其中 `T` 与 `E` 都实现了 `IntoContents`, 那就会自动标记成功或者失败。

### 管理多个服务
在很多情况下你需要把不同类型的服务管理在一个集合当中，你可以调用 `into_dyn` 来把他们都转化成动态类型。
```rust, ignore
let service = service.into_dyn();
```


### 用例
查看 [用例文件夹](examples/README.md)

### Features
- `client`: 使用客户端sdk
- `server`: 使用服务端sdk


## 相关资源
- [MCP Specification](https://spec.modelcontextprotocol.io/specification/2024-11-05/)

- [Schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.ts)
