<style>
.rustdoc-hidden { display: none; }
</style>

<div class="rustdoc-hidden">

# rmcp-macros

[![Crates.io](https://img.shields.io/crates/v/rmcp-macros.svg)](https://crates.io/crates/rmcp-macros)
[![Documentation](https://docs.rs/rmcp-macros/badge.svg)](https://docs.rs/rmcp-macros)

</div>

Procedural macros for the [RMCP](../rmcp) SDK. Most users should depend on `rmcp` with the `macros` feature (enabled by default) rather than using this crate directly.

For **getting started** and **full MCP feature documentation**, see the [main README](../../README.md).

## Available Macros

| Macro | Description |
|-------|-------------|
| [`#[tool]`][tool] | Mark a function as an MCP tool handler |
| [`#[tool_router]`][tool_router] | Generate a tool router from an impl block |
| [`#[tool_handler]`][tool_handler] | Generate `call_tool` and `list_tools` handler methods |
| [`#[prompt]`][prompt] | Mark a function as an MCP prompt handler |
| [`#[prompt_router]`][prompt_router] | Generate a prompt router from an impl block |
| [`#[prompt_handler]`][prompt_handler] | Generate `get_prompt` and `list_prompts` handler methods |
| [`#[task_handler]`][task_handler] | Wire up the task lifecycle on top of an `OperationProcessor` |

[tool]: https://docs.rs/rmcp-macros/latest/rmcp_macros/attr.tool.html
[tool_router]: https://docs.rs/rmcp-macros/latest/rmcp_macros/attr.tool_router.html
[tool_handler]: https://docs.rs/rmcp-macros/latest/rmcp_macros/attr.tool_handler.html
[prompt]: https://docs.rs/rmcp-macros/latest/rmcp_macros/attr.prompt.html
[prompt_router]: https://docs.rs/rmcp-macros/latest/rmcp_macros/attr.prompt_router.html
[prompt_handler]: https://docs.rs/rmcp-macros/latest/rmcp_macros/attr.prompt_handler.html
[task_handler]: https://docs.rs/rmcp-macros/latest/rmcp_macros/attr.task_handler.html

## Quick Example

```rust,ignore
use rmcp::{tool, tool_router, tool_handler, ServerHandler};

#[derive(Clone)]
struct MyServer;

#[tool_router]
impl MyServer {
    #[tool(description = "Say hello")]
    async fn hello(&self) -> String {
        "Hello, world!".into()
    }
}

#[tool_handler]
impl ServerHandler for MyServer {}
```

See the [full documentation](https://docs.rs/rmcp-macros) for detailed usage of each macro.

## License

This project is licensed under the terms specified in the repository's [LICENSE](../../LICENSE) file.
