# Quick Start With Claude Desktop

1. **Build the Server (Counter Example)**
    ```sh
    cargo build --release --example servers_std_io
    ```
    This builds a standard input/output MCP server binary.

2. **Add or update this section in your** `~/.config/claude-desktop/config.toml` (Linux) or `~/Library/Application Support/Claude/claude_desktop_config.json` (macOS)
    ```json
    {
        "mcpServers": {
            "counter": {
            "command": "PATH-TO/rust-sdk/target/release/examples/servers_std_io.exe",
            "args": []
            }
        }
    }
    ```

3. **Ensure that the MCP UI elements appear in Claude Desktop**
    The MCP UI elements will only show up in Claude for Desktop if at least one server is properly configured.

4. **Once Claude Desktop is running, try chatting:**
    ```text
    counter.say_hello
    ```
    Or test other tools like:
    ```text
    counter.increment
    counter.get_value
    counter.sum {"a": 3, "b": 4}
    ```

# Client Examples

- [Client SSE](clients/src/sse.rs), using reqwest and eventsource-client.
- [Client stdio](clients/src/std_io.rs), using tokio to spawn child process.
- [Everything](clients/src/everything_stdio.rs), test with `@modelcontextprotocol/server-everything`
- [Collection](clients/src/collection.rs), How to transpose service into dynamic object, so they will have a same type.

# Server Examples

- [Server SSE](servers/src/axum.rs), using axum as web server.
- [Server stdio](servers/src/std_io.rs), using tokio async io.

# Transport Examples

- [Tcp](transport/src/tcp.rs)
- [Transport on http upgrade](transport/src/http_upgrade.rs)
- [Unix Socket](transport/src/unix_socket.rs)
- [Websocket](transport/src/websocket.rs)

# Integration

- [Rig](examples/rig-integration) A stream chatbot with rig

# WASI

- [WASI-P2 runtime](wasi) How it works with wasip2

## Use Mcp Inspector

```sh
npx @modelcontextprotocol/inspector
```
