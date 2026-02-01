# rmcp-macros

`rmcp-macros` is a procedural macro library for the Rust Model Context Protocol (RMCP) SDK, providing macros that facilitate the development of RMCP applications.

## Features

This library primarily provides the following macros:

- `#[tool]`: Mark an async/sync function as an RMCP tool and generate metadata + schema glue
- `#[tool_router]`: Collect all `#[tool]` functions in an impl block into a router value
- `#[tool_handler]`: Implement the `call_tool` and `list_tools` entry points by delegating to a router expression
- `#[task_handler]`: Wire up the task lifecycle (list/enqueue/get/cancel) on top of an `OperationProcessor`

## Usage

### tool

This macro is used to mark a function as a tool handler.

This will generate a function that return the attribute of this tool, with type `rmcp::model::Tool`.

#### Tool attributes

| field             | type                       | usage |
| :-                | :-                         | :-    |
| `name`            | `String`                   | The name of the tool. If not provided, it defaults to the function name. |
| `description`     | `String`                   | A description of the tool. The document of this function will be used. |
| `input_schema`    | `Expr`                     | A JSON Schema object defining the expected parameters for the tool. If not provide, if will use the json schema of its argument with type `Parameters<T>` |
| `annotations`     | `ToolAnnotationsAttribute` | Additional tool information. Defaults to `None`. |

#### Tool example

```rust
#[tool(name = "my_tool", description = "This is my tool", annotations(title = "我的工具", read_only_hint = true))]
pub async fn my_tool(param: Parameters<MyToolParam>) {
    // handling tool request
}
```

### tool_router

This macro is used to generate a tool router based on functions marked with `#[rmcp::tool]` in an implementation block.

It creates a function that returns a `ToolRouter` instance.

In most case, you need to add a field for handler to store the router information and initialize it when creating handler, or store it with a static variable.

#### Router attributes

| field     | type          | usage |
| :-        | :-            | :-    |
| `router`  | `Ident`       | The name of the router function to be generated. Defaults to `tool_router`. |
| `vis`     | `Visibility`  | The visibility of the generated router function. Defaults to empty. |

#### Router example

```rust
#[tool_router]
impl MyToolHandler {
    #[tool]
    pub fn my_tool() {
        
    }

    pub fn new() -> Self {
        Self {
            // the default name of tool router will be `tool_router`
            tool_router: Self::tool_router(),
        }
    }
}
```

Or specify the visibility and router name, which would be helpful when you want to combine multiple routers into one:

```rust
mod a {
    #[tool_router(router = tool_router_a, vis = "pub")]
    impl MyToolHandler {
        #[tool]
        fn my_tool_a() {
            
        }
    }
}

mod b {
    #[tool_router(router = tool_router_b, vis = "pub")]
    impl MyToolHandler {
        #[tool]
        fn my_tool_b() {
            
        }
    }
}

impl MyToolHandler {
    fn new() -> Self {
        Self {
            tool_router: self::tool_router_a() + self::tool_router_b(),
        }
    }
}
```

### tool_handler

This macro will generate the handler for `tool_call` and `list_tools` methods in the implementation block, by using an existing `ToolRouter` instance.

#### Handler attributes

| field     | type          | usage |
| :-        | :-            | :-    |
| `router`  | `Expr`        | The expression to access the `ToolRouter` instance. Defaults to `self.tool_router`. |

#### Handler example

```rust
#[tool_handler]
impl ServerHandler for MyToolHandler {
    // ...implement other handler
}
```

or using a custom router expression:

```rust
#[tool_handler(router = self.get_router().await)]
impl ServerHandler for MyToolHandler {
   // ...implement other handler
}
```

#### Handler expansion

This macro will be expended to something like this:

```rust
impl ServerHandler for MyToolHandler {
       async fn call_tool(
        &self,
        request: CallToolRequestParam,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let tcc = ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        let items = self.tool_router.list_all();
        Ok(ListToolsResult::with_all_items(items))
    }
}
```

### task_handler

This macro wires the task lifecycle endpoints (`list_tasks`, `enqueue_task`, `get_task`, `cancel_task`) to an implementation of `OperationProcessor`. It keeps the handler lean by delegating scheduling, status tracking, and cancellation semantics to the processor.

#### Task handler attributes

| field        | type   | usage |
| :-           | :-     | :-    |
| `processor`  | `Expr` | Expression that yields an `Arc<dyn OperationProcessor>` (or compatible trait object). Defaults to `self.processor.clone()`. |

#### Task handler example

```rust
#[derive(Clone)]
pub struct TaskHandler {
    processor: Arc<dyn OperationProcessor<RoleServer> + Send + Sync>,
}

#[task_handler(processor = self.processor.clone())]
impl ServerHandler for TaskHandler {}
```

#### Task handler expansion

At expansion time the macro implements the task-specific handler methods by forwarding to the processor expression, roughly equivalent to:

```rust
impl ServerHandler for TaskHandler {
    async fn list_tasks(&self, request: TaskListRequest, ctx: RequestContext<RoleServer>) -> Result<TaskListResult, rmcp::ErrorData> {
        self.processor.list_tasks(request, ctx).await
    }

    async fn enqueue_task(&self, request: TaskEnqueueRequest, ctx: RequestContext<RoleServer>) -> Result<TaskEnqueueResult, rmcp::ErrorData> {
        self.processor.enqueue_task(request, ctx).await
    }

    // get_task and cancel_task are generated in the same manner.
}
```


## Advanced Features

- Support for custom tool names and descriptions
- Automatic generation of tool descriptions from documentation comments
- JSON Schema generation for tool parameters

## License

Please refer to the LICENSE file in the project root directory.
