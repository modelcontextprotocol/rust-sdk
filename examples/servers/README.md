# MCP Server Examples

This directory contains Model Context Protocol (MCP) server examples implemented in Rust. These examples demonstrate how to create MCP servers using different transport methods and how to implement various server capabilities including tools, resources, prompts, and authentication.

## Example List

### Counter Standard I/O Server (`counter_stdio.rs`)

A basic MCP server that communicates using standard input/output transport.

- Provides a simple counter tool with increment, decrement, and get_value operations
- Demonstrates basic tool implementation and stdio transport

### Memory Standard I/O Server (`memory_stdio.rs`)

A minimal server example using stdio transport.

- Lightweight server implementation
- Demonstrates basic server setup patterns
- Good starting point for custom server development

### Counter Streamable HTTP Server (`counter_streamhttp.rs`)

A server using streamable HTTP transport for MCP communication, with axum.

- Runs on HTTP with streaming capabilities
- Provides counter tools via HTTP streaming
- Demonstrates streamable HTTP transport configuration

### Counter Streamable HTTP Server with Hyper (`counter_hyper_streamable_http.rs`)

A server using streamable HTTP transport for MCP communication, with hyper.

- Runs on HTTP with streaming capabilities
- Provides counter tools via HTTP streaming
- Demonstrates streamable HTTP transport configuration

### Elicitation Demo (`elicitation_stdio.rs`)

A working MCP server demonstrating elicitation for user name collection.

- Real MCP server using rmcp library
- `context.peer.elicit::<T>()` API usage
- Type-safe elicitation with `elicit_safe!` macro
- JSON Schema validation with schemars
- Tools: `greet_user` (collects name), `reset_name` (clears stored name)

### Prompt Standard I/O Server (`prompt_stdio.rs`)

A server demonstrating the prompt framework capabilities.

- Shows how to implement prompts in MCP servers
- Provides code review and debugging prompts
- Demonstrates prompt argument handling with JSON schema
- Uses standard I/O transport
- Good example of prompt implementation patterns

### Progress Demo Server (`progress_demo.rs`)

A server that demonstrates progress notifications during long-running operations.

- Provides a stream_processor tool that generates progress notifications
- Demonstrates progress notifications during long-running operations
- Can be run with `cargo run --example servers_progress_demo -- {stdio|http|all}`

### Simple Auth Streamable HTTP Server (`simple_auth_streamhttp.rs`)

A server demonstrating simple token-based authentication with streamable HTTP transport.

- Uses bearer token authentication via Authorization header
- Provides `/api/token/{id}` endpoint to get demo tokens
- Protected MCP endpoint at `/mcp`
- Shows how to add auth middleware to streamable HTTP services

### Complex Auth Streamable HTTP Server (`complex_auth_streamhttp.rs`)

A full OAuth 2.0 authorization server implementation with streamable HTTP MCP transport.

- Complete OAuth 2.0 authorization code flow
- Client registration endpoint
- Authorization server metadata discovery
- Protected MCP endpoint with token validation
- Demonstrates building a production-like auth server

## How to Run

Each example can be run using Cargo:

```bash
# Run the counter standard I/O server
cargo run --example servers_counter_stdio

# Run the memory standard I/O server
cargo run --example servers_memory_stdio

# Run the counter streamable HTTP server
cargo run --example servers_counter_streamhttp

# Run the elicitation standard I/O server
cargo run --example servers_elicitation_stdio

# Run the prompt standard I/O server
cargo run --example servers_prompt_stdio

# Run the simple auth streamable HTTP server
cargo run --example servers_simple_auth_streamhttp

# Run the complex auth streamable HTTP server
cargo run --example servers_complex_auth_streamhttp
```

## Testing with MCP Inspector

Many of these servers can be tested using the MCP Inspector tool:
See [inspector](https://github.com/modelcontextprotocol/inspector)

## Dependencies

These examples use the following main dependencies:

- `rmcp`: Rust implementation of the MCP server library
- `tokio`: Asynchronous runtime
- `serde` and `serde_json`: For JSON serialization and deserialization
- `tracing` and `tracing-subscriber`: For logging
- `anyhow`: Error handling
- `axum`: Web framework for HTTP-based transports
- `tokio-util`: Utilities for async programming
- `schemars`: JSON Schema generation (used in elicitation examples)

## Common Module

The `common/` directory contains shared code used across examples:

- `counter.rs`: Counter tool implementation with MCP server traits
- `calculator.rs`: Calculator tool examples
- `generic_service.rs`: Generic service implementations

This modular approach allows for code reuse and demonstrates how to structure larger MCP server applications.
