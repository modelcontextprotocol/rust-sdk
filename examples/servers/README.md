# MCP Server Examples

This directory contains Model Context Protocol (MCP) server examples implemented in Rust. These examples demonstrate how to create MCP servers using different transport methods and how to implement various server capabilities including tools, resources, prompts, and authentication.

## Example List

### Counter Standard I/O Server (`counter_stdio.rs`)

A basic MCP server that communicates using standard input/output transport.

- Provides a simple counter tool with increment, decrement, and get_value operations
- Demonstrates basic tool implementation and stdio transport

### Counter SSE Server (`counter_sse.rs`)

A server that provides counter functionality using Server-Sent Events (SSE) transport.

- Runs on `http://127.0.0.1:8000/sse` by default
- Provides the same counter tools as the stdio version
- Demonstrates SSE transport setup with graceful shutdown
- Can be accessed via web browsers or SSE-compatible clients

### Counter SSE Direct Server (`counter_sse_directly.rs`)

A minimal SSE server implementation showing direct SSE server usage.

- Simplified version of the SSE server
- Demonstrates basic SSE server configuration
- Provides counter functionality with minimal setup

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

### Complex OAuth SSE Server (`complex_auth_sse.rs`)

A comprehensive example demonstrating OAuth 2.0 integration with MCP servers.

- Full OAuth 2.0 authorization server implementation
- Client registration and token management
- User authorization flow with web interface
- Token validation middleware
- Integrated with MCP SSE transport
- Demonstrates enterprise-grade authentication patterns

### Simple OAuth SSE Server (`simple_auth_sse.rs`)

A simplified OAuth example showing basic token-based authentication.

- Basic token store and validation
- Authorization middleware for SSE endpoints
- Token generation API
- Simplified authentication flow
- Good starting point for adding authentication to MCP servers

### Elicitation Demo (`elicitation_stdio.rs`)

A working MCP server demonstrating elicitation for user name collection.

- Real MCP server using rmcp library
- `context.peer.elicit::<T>()` API usage
- Type-safe elicitation with `elicit_safe!` macro  
- JSON Schema validation with schemars
- Tools: `greet_user` (collects name), `reset_name` (clears stored name)

## How to Run

Each example can be run using Cargo:

```bash
# Run the counter standard I/O server
cargo run --example servers_counter_stdio

# Run the counter SSE server
cargo run --example servers_counter_sse

# Run the counter SSE direct server
cargo run --example servers_counter_sse_directly

# Run the memory standard I/O server
cargo run --example servers_memory_stdio

# Run the counter streamable HTTP server
cargo run --example servers_counter_streamhttp

# Run the complex OAuth SSE server
cargo run --example servers_complex_auth_sse

# Run the simple OAuth SSE server
cargo run --example servers_simple_auth_sse

# Run the elicitation standard I/O server
cargo run --example servers_elicitation_stdio
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
- `askama`: Template engine (used in OAuth examples)
- `tower-http`: HTTP middleware (used for CORS in OAuth examples)
- `uuid`: UUID generation (used in OAuth examples)
- `chrono`: Date and time handling (used in OAuth examples)
- `rand`: Random number generation (used in OAuth examples)
- `schemars`: JSON Schema generation (used in elicitation examples)

## Common Module

The `common/` directory contains shared code used across examples:

- `counter.rs`: Counter tool implementation with MCP server traits
- `calculator.rs`: Calculator tool examples
- `generic_service.rs`: Generic service implementations

This modular approach allows for code reuse and demonstrates how to structure larger MCP server applications.
