```sh
cargo build -p wasi --target wasm32-wasip2
 npx @modelcontextprotocol/inspector wasmtime target/wasm32-wasip2/debug/wasi.wasm
```