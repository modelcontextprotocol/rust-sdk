# RMCP
[![Crates.io Version](https://img.shields.io/crates/v/rmcp)](https://crates.io/crates/rmcp)
![Release status](https://github.commodelcontextprotocol/rust-sdk/actions/workflows/release.yml/badge.svg)
[![docs.rs](https://img.shields.io/docsrs/rmcp)](https://docs.rs/rmcp/latest/rmcp)

一个基于tokio异步运行时的官方Model Context Protocol SDK实现。

## 使用

### 导入
```toml
rmcp = { version = "0.1", features = ["server"] }
## 或者开发者频道
rmcp = { git = "https://github.com/modelcontextprotocol/rust-sdk", branch = "main" }
```

### 快速上手
一行代码启动客户端：
```rust
use rmcp::{ServiceExt, transport::{TokioChildProcess, ConfigureCommandExt}};
use tokio::process::Command;

let client = ().serve(TokioChildProcess::new(Command::new("npx").configure(|cmd| {
    cmd.arg("-y").arg("@modelcontextprotocol/server-everything");
}))?).await?;
```

#### 1. 构建传输层

```rust, ignore
use tokio::io::{stdin, stdout};
let transport = (stdin(), stdout());
```

传输层类型必须实现 [`IntoTransport`](crate::transport::IntoTransport) trait, 这个特性允许分割成一个sink和一个stream。

对于客户端, Sink 的 Item 是 [`ClientJsonRpcMessage`](crate::model::ClientJsonRpcMessage)， Stream 的 Item 是 [`ServerJsonRpcMessage`](crate::model::ServerJsonRpcMessage)

对于服务端, Sink 的 Item 是 [`ServerJsonRpcMessage`](crate::model::ServerJsonRpcMessage)， Stream 的 Item 是 [`ClientJsonRpcMessage`](crate::model::ClientJsonRpcMessage)

##### 这些类型自动实现了 [`IntoTransport`](crate::transport::IntoTransport) trait
1. 已经同时实现了 [`Sink`](futures::Sink) 和 [`Stream`](futures::Stream) trait的类型。
2. 由sink `Tx` 和 stream `Rx`组成的元组: `(Tx, Rx)`。
3. 同时实现了 [`tokio::io::AsyncRead`] 和 [`tokio::io::AsyncWrite`] trait的类型。
4. 由 [`tokio::io::AsyncRead`] `R `和 [`tokio::io::AsyncWrite`] `W` 组成的元组:  `(R, W)`。

例如，你可以看到我们如何轻松地通过TCP流或http升级构建传输层。 [examples](examples/README.md)

#### 2. 构建服务
你可以通过 [`ServerHandler`](crates/rmcp/src/handler/server.rs) 或 [`ClientHandler`](crates/rmcp/src/handler/client.rs) 轻松构建服务

```rust, ignore
let service = common::counter::Counter::new();
```

#### 3. 把他们组装到一起
```rust, ignore
// 这里会自动完成初始化流程
let server = service.serve(transport).await?;
```

#### 4. 与服务交互
一旦服务初始化完成，你可以发送请求或通知：

```rust, ignore
// 请求
let roots = server.list_roots().await?;

// 或发送通知
server.notify_cancelled(...).await?;
```

#### 5. 等待服务关闭
```rust, ignore
let quit_reason = server.waiting().await?;
// 或取消它
let quit_reason = server.cancel().await?;
```

### 使用宏来声明工具
使用 `toolbox` 和 `tool` 宏来快速创建工具。

请看这个[文件](examples/servers/src/common/calculator.rs)。
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
你要做的唯一事情就是确保函数的返回类型实现了 `IntoCallToolResult`。

你可以为返回类型实现 `IntoContents`，那么返回值将自动标记为成功。

如果返回类型是 `Result<T, E>`，其中 `T` 与 `E` 都实现了 `IntoContents`，那也是可以的。

### 管理多个服务
在很多情况下你需要在一个集合中管理多个服务，你可以调用 `into_dyn` 来将服务转换为相同类型。
```rust, ignore
let service = service.into_dyn();
```


### 示例
查看 [examples](examples/README.md)

### 功能特性
- `client`: 使用客户端sdk
- `server`: 使用服务端sdk
- `macros`: 宏默认
#### 传输层
- `transport-io`: 服务端标准输入输出传输
- `transport-sse-server`: 服务端SSE传输
- `transport-child-process`: 客户端标准输入输出传输
- `transport-sse`: 客户端SSE传输

## 相关资源
- [MCP Specification](https://spec.modelcontextprotocol.io/specification/2024-11-05/)

- [Schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.ts)