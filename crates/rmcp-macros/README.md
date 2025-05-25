# rmcp-macros

`rmcp-macros` is a procedural macro library for the Rust Model Context Protocol (RMCP) SDK, providing macros that facilitate the development of RMCP applications.

## Features

This library primarily provides the following macros:

- `#[tool]`: Used to mark functions as RMCP tools, automatically generating necessary metadata and invocation mechanisms

## Usage

### Tool Macro

Mark a function as a tool:

```rust ignore
#[tool]
fn calculator(&self, #[tool(param)] a: i32, #[tool(param)] b: i32) -> Result<CallToolResult, Error> {
    // Implement tool functionality
    Ok(CallToolResult::success(vec![Content::text((a + b).to_string())]))
}

```

Use on an impl block to automatically register multiple tools:

```rust ignore
#[tool(tool_box)]
impl MyHandler {
    #[tool]
    fn tool1(&self) -> Result<CallToolResult, Error> {
        // Tool 1 implementation
    }
    
    #[tool]
    fn tool2(&self) -> Result<CallToolResult, Error> {
        // Tool 2 implementation
    }
}
```



## Advanced Features

- Support for parameter aggregation (`#[tool(aggr)]`)
- Support for custom tool names and descriptions
- Automatic generation of tool descriptions from documentation comments
- JSON Schema generation for tool parameters

## License

Please refer to the LICENSE file in the project root directory. 