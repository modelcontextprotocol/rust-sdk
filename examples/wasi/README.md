# Example for WASI-p2

Build:

```sh
cargo build -p wasi-mcp-example --target wasm32-wasip2
```

Run:

```
npx @modelcontextprotocol/inspector wasmtime target/wasm32-wasip2/debug/wasi_mcp_example.wasm
```

*Note:* Change `wasmtime` to a different installed run time, if needed.

The printed URL of the MCP inspector can be opened and a connection to the module established via `STDIO`.