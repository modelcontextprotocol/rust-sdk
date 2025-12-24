# AI CLI Integration Guide for Rust MCP SDK

This guide shows how to integrate MCP servers built with the Rust SDK into AI CLI tools like Claude Code CLI, Codex CLI, and Gemini CLI.

## Overview

The Model Context Protocol (MCP) allows AI tools to interact with external services. This guide covers setting up Rust-based MCP servers to work with popular AI CLI tools.

## Prerequisites

- Rust 1.70+ and Cargo
- One or more AI CLI tools: Claude Code CLI, Codex CLI, or Gemini CLI
- Basic familiarity with command-line tools

## Building Your MCP Server

First, create your MCP server using the Rust SDK. Here's a complete example for file operations:

```rust
// file_operations_stdio.rs
use anyhow::Result;
use rmcp::{ServiceExt, transport::stdio, ErrorData as McpError, RoleServer, ServerHandler};
use rmcp::handler::server::{
    router::tool::ToolRouter,
    wrapper::Parameters,
};
use rmcp::model::*;
use rmcp::{tool, tool_handler, tool_router};
// ... (see file_operations_stdio.rs for complete implementation)
```

Build your server:

```bash
cargo build --release
```

## CLI Tool Integration

### Claude Code CLI

Claude Code CLI requires wrapper scripts because it cannot pass complex arguments directly to the MCP server binary.

#### Step 1: Create a wrapper script

```bash
# Create wrapper script
cat > claude-file-ops.sh << 'EOF'
#!/bin/bash
exec ./target/release/file-operations-mcp
EOF

# Make it executable
chmod +x claude-file-ops.sh
```

#### Step 2: Add to Claude Code CLI

```bash
claude mcp add file-ops ./claude-file-ops.sh
```

#### Step 3: Verify the configuration

```bash
claude mcp list | grep file-ops
```

### Codex CLI

Codex CLI can pass arguments directly to the MCP server binary.

```bash
# Add the MCP server directly
codex mcp add file-ops -- ./target/release/file-operations-mcp

# Verify the configuration
codex mcp list | grep file-ops
```

### Gemini CLI

Gemini CLI works similarly to Claude Code CLI and requires a wrapper script.

#### Step 1: Create a wrapper script

```bash
# Create wrapper script  
cat > gemini-file-ops.sh << 'EOF'
#!/bin/bash
exec ./target/release/file-operations-mcp
EOF

# Make it executable
chmod +x gemini-file-ops.sh
```

#### Step 2: Add to Gemini CLI

```bash
gemini mcp add file-ops ./gemini-file-ops.sh
```

#### Step 3: Verify the configuration

```bash
gemini mcp list | grep file-ops
```

## Testing Your Integration

Once configured, you can test your MCP server in any of the CLI tools:

### Example Commands to Test

#### File Operations
```
# Reading files
"Read the contents of package.json"

# Writing files  
"Write 'Hello, World!' to a file called test.txt"

# Command execution
"Run the command 'ls -la' to see the current directory contents"
```

#### Basic Verification
```
# List available tools
"What tools do you have available?"

# Test basic connectivity
"Use the file operations tools to create a simple test file"
```

## Troubleshooting

### Debug Logging

The Rust MCP SDK logs to stderr by default. You can enable debug logging:

```bash
# Enable debug logging when running manually
RUST_LOG=debug ./target/release/file-operations-mcp

# Or set the environment variable in your wrapper script
cat > debug-wrapper.sh << 'EOF'
#!/bin/bash
export RUST_LOG=debug
exec ./target/release/file-operations-mcp
EOF
```

### Common Issues

#### "Failed to connect" / "Transport closed"

1. **Check binary permissions**: Ensure your binary is executable
   ```bash
   chmod +x ./target/release/file-operations-mcp
   ```

2. **Verify wrapper script**: Make sure wrapper scripts are executable
   ```bash
   chmod +x ./claude-file-ops.sh
   ```

3. **Test manually**: Run the server manually to check for startup errors
   ```bash
   echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}' | ./target/release/file-operations-mcp
   ```

#### "Unexpected response type"

This usually indicates a protocol format issue. Ensure you're:
- Using the latest version of the Rust SDK
- Properly implementing the `ServerHandler` trait
- Returning correct response formats from your tools

#### "MCP startup failed: handshaking failed"

1. **Restart the CLI**: Sometimes cached configurations cause issues
2. **Remove and re-add**: Remove the MCP server configuration and add it again
   ```bash
   # For Claude Code CLI
   claude mcp remove file-ops
   claude mcp add file-ops ./claude-file-ops.sh
   ```

### Manual Testing

You can test your MCP server manually using JSON-RPC messages:

```bash
# Test initialization
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{"tools":{"listChanged":true}},"clientInfo":{"name":"test","version":"1.0"}}}' | ./target/release/file-operations-mcp

# Test tools list
echo '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' | ./target/release/file-operations-mcp

# Test tool execution
echo '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"read_file","arguments":{"path":"./Cargo.toml"}}}' | ./target/release/file-operations-mcp
```

## Configuration Templates

### Cargo.toml for MCP Projects

```toml
[package]
name = "my-mcp-server"
version = "0.1.0"
edition = "2021"

[dependencies]
anyhow = "1.0"
rmcp = { git = "https://github.com/modelcontextprotocol/rust-sdk", features = ["server", "transport-io", "macros"] }
schemars = { version = "0.8", features = ["chrono"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
tokio = { version = "1.0", features = ["full"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

### Universal Wrapper Script Template

```bash
#!/bin/bash
# Universal MCP server wrapper script template

# Set debug logging if needed
# export RUST_LOG=debug

# Change to the directory containing your binary
cd "$(dirname "$0")"

# Execute the MCP server
exec ./target/release/my-mcp-server
```

## Best Practices

### 1. Error Handling
Always handle errors gracefully and return descriptive error messages:

```rust
match fs::read_to_string(&path) {
    Ok(content) => Ok(CallToolResult::success(vec![Content::text(content)])),
    Err(e) => Ok(CallToolResult {
        content: vec![Content::text(format!("Failed to read file '{}': {}", path, e))],
        is_error: Some(true),
        _meta: None,
    }),
}
```

### 2. Logging
Log to stderr to avoid interfering with the MCP protocol:

```rust
tracing_subscriber::fmt()
    .with_env_filter(EnvFilter::from_default_env())
    .with_writer(std::io::stderr)  // Important: use stderr
    .with_ansi(false)
    .init();
```

### 3. Tool Descriptions
Provide clear, descriptive tool descriptions:

```rust
#[tool(description = "Execute a shell command with optional working directory")]
async fn execute_command(&self, args: ExecuteCommandArgs) -> Result<CallToolResult, McpError> {
    // implementation
}
```

### 4. Input Validation
Validate inputs and provide helpful error messages:

```rust
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ReadFileArgs {
    /// Path to the file to read (must be a valid file path)
    pub path: String,
}
```

## Security Considerations

### File Access
- Consider implementing path restrictions for file operations
- Validate file paths to prevent directory traversal attacks
- Be cautious with file write operations

### Command Execution
- Consider restricting allowed commands
- Be aware that command execution tools can be powerful and potentially dangerous
- Consider running in sandboxed environments for production use

### Example with Basic Security

```rust
fn is_safe_path(path: &str) -> bool {
    // Prevent directory traversal
    !path.contains("..") && !path.starts_with("/")
}

#[tool(description = "Read a file with basic path validation")]
async fn safe_read_file(
    &self,
    Parameters(args): Parameters<ReadFileArgs>,
) -> Result<CallToolResult, McpError> {
    if !is_safe_path(&args.path) {
        return Ok(CallToolResult {
            content: vec![Content::text("Invalid file path")],
            is_error: Some(true),
            _meta: None,
        });
    }
    
    // ... rest of implementation
}
```

## Advanced Usage

### Custom Transport
While this guide focuses on stdio transport, the Rust SDK supports other transports:

```rust
// HTTP transport example
use rmcp::transport::streamable_http_server;

let service = FileOperations::new()
    .serve(streamable_http_server("127.0.0.1:3000"))
    .await?;
```

### Multiple Tool Routers
You can combine multiple tool routers for complex servers:

```rust
#[derive(Clone)]
pub struct CombinedServer {
    file_ops: FileOperations,
    other_tools: OtherTools,
    tool_router: ToolRouter<CombinedServer>,
}

// Implement tools for both routers...
```

## Examples Repository

For more complete examples, see:
- [File Operations Example](../servers/file_operations_stdio.rs)
- [Official Examples](../servers/)

## Contributing

To contribute improvements to this guide or report issues:
1. Open an issue in the rust-sdk repository
2. Submit a pull request with improvements
3. Join the MCP community discussions

## Related Resources

- [Model Context Protocol Specification](https://modelcontextprotocol.io/)
- [Rust MCP SDK Documentation](https://docs.rs/rmcp/)
- [Claude Code CLI Documentation](https://docs.anthropic.com/en/docs/claude-code)
- [Official MCP Servers](https://github.com/modelcontextprotocol/servers)