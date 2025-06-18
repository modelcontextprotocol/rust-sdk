# MCP Client Examples

This directory contains Model Context Protocol (MCP) client examples implemented in Rust. These examples demonstrate how to communicate with MCP servers using different transport methods and how to use various client APIs.

## Example List


### SSE Client (`sse.rs`)

A client that communicates with an MCP server using Server-Sent Events (SSE) transport.

- Connects to an MCP server running at `http://localhost:8000/sse`
- Retrieves server information and list of available tools
- Calls a tool named "increment"

### Git Standard I/O Client (`git_stdio.rs`)

A client that communicates with a Git-related MCP server using standard input/output.

- Launches the `uvx mcp-server-git` command as a child process
- Retrieves server information and list of available tools
- Calls the `git_status` tool to check the Git status of the current directory

### Streamable HTTP Client (`streamable_http.rs`)

A client that communicates with an MCP server using HTTP streaming transport.
- Connects to an MCP server running at `http://localhost:8000`
- Retrieves server information and list of available tools
- Calls a tool named "increment"

### Full-Featured Standard I/O Client (`everything_stdio.rs`)

An example demonstrating all MCP client capabilities.

- Launches `npx -y @modelcontextprotocol/server-everything` as a child process
- Retrieves server information and list of available tools
- Calls various tools, including "echo" and "longRunningOperation"
- Lists and reads available resources
- Lists and retrieves simple and complex prompts
- Lists available resource templates

### Client Collection (`collection.rs`)

An example showing how to manage multiple MCP clients.

- Creates 10 clients connected to Git servers
- Stores these clients in a HashMap
- Performs the same sequence of operations on each client
- Uses `into_dyn()` to convert services to dynamic services

### OAuth Client (`auth/oauth_client.rs`)

A client demonstrating how to authenticate with an MCP server using OAuth.

- Starts a local HTTP server to handle OAuth callbacks
- Initializes the OAuth state machine and begins the authorization flow
- Displays the authorization URL and waits for user authorization
- Establishes an authorized connection to the MCP server using the acquired access token
- Demonstrates how to use the authorized connection to retrieve available tools and prompts


### Sampling Standard I/O Client (`sampling_stdio.rs`)

A client demonstrating how to use the sampling tool.

- Launches the server example `servers_sampling_stdio`
- Connects to the server
- Retrieves server information and list of available tools
- Calls the `ask_llm` tool

### Progress Test Client (`progress_test_client.rs`)

A client that communicates with an MCP server using progress notifications.

- Launches the `cargo run --example clients_progress_client -- --transport {stdio|sse|http|all}` to test the progress notifications
- Connects to the server using different transport methods
- Tests the progress notifications
- The sse and http should run the server first


## How to Run

Each example can be run using Cargo:

```bash
# Run the SSE client example
cargo run --example clients_sse

# Run the Git standard I/O client example
cargo run --example clients_git_stdio

# Run the streamable HTTP client example
cargo run --example clients_streamable_http

# Run the full-featured standard I/O client example
cargo run --example clients_everything_stdio

# Run the client collection example
cargo run --example clients_collection

# Run the OAuth client example
cargo run --example clients_oauth_client

# Run the sampling standard I/O client example
cargo run --example clients_sampling_stdio
```

## Dependencies

These examples use the following main dependencies:

- `rmcp`: Rust implementation of the MCP client library
- `tokio`: Asynchronous runtime
- `serde` and `serde_json`: For JSON serialization and deserialization
- `tracing` and `tracing-subscriber`: For logging, not must, only for logging
- `anyhow`: Error handling, not must, only for error handling
- `axum`: For the OAuth callback HTTP server (used only in the OAuth example)
- `reqwest`: HTTP client library (used for OAuth and streamable HTTP transport)
