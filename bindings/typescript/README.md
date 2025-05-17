 # TypeScript Bindings Setup & Example Usage

## Prerequisites
- Rust (latest stable, with cargo)
- Node.js (latest LTS, with npm)
- TypeScript (install via `npm install -g typescript`)

## Build the TypeScript Bindings
1. Navigate to the TypeScript bindings directory:
   ```bash
   cd bindings/typescript
   ```
2. Install dependencies and build the project:
   ```bash
   npm install
   npm run build
   ```
## Note on Cargo.toml
If you encounter an error indicating that the `Cargo.toml` of `bindings/python` is not present in the root `Cargo.toml`, and you don't want to build Python and TypeScript together, you can edit the root `Cargo.toml` to remove the Python bindings (bindings/python) entry. 

## Run the Rust Server
In a separate terminal, start the Rust SDK server (example using the rmcp crate):
```bash
cd examples/servers
cargo run --example servers_axum
```

## Link the Module Locally
If the `rmcp-typescript` module is not published to npm, you can link it locally:
1. Navigate to the TypeScript bindings directory:
   ```bash
   cd bindings/typescript
   ```
2. Link the module:
   ```bash
   npm link
   ```
3. Navigate to the examples/clients directory and link the module:
   ```bash
   cd examples/clients
   npm link rmcp-typescript
   ```

## Run the TypeScript SSE Client Example
With the server running, in another terminal:
```bash
cd bindings/typescript/examples/clients/src
npx tsx sse.ts
```

This will connect to the running Rust server and demonstrate the SSE client functionality.

