//! # rmcp_typescript
//!
//! TypeScript/JavaScript bindings for the RMCP SDK, powered by napi-rs.
//!
//! This crate exposes a Node.js-compatible API for interacting with RMCP services from TypeScript or JavaScript.
//!
//! ## Usage Example (TypeScript)
//!
//! ```typescript
//! import {
//!   JsTransport,
//!   JsSseTransport,
//!   JsClientInfo,
//!   JsImplementation,
//!   JsClientCapabilities,
//!   JsExperimentalCapabilities,
//!   JsRootsCapabilities,
//! } from 'rmcp-typescript';
//!
//! // 1. Create an SSE transport
//! const sseEndpoint = 'http://localhost:8000/sse';
//! const sseTransport = await JsSseTransport.start(sseEndpoint);
//! const transport = JsTransport.fromSse(sseTransport);
//!
//! // 2. Define client info and capabilities
//! const experimental = JsExperimentalCapabilities.new({});
//! const roots = new JsRootsCapabilities();
//! const capabilities = new JsClientCapabilities(experimental, roots, null);
//! const impl = new JsImplementation('typescript-client', '0.1.0');
//! const clientInfo = new JsClientInfo('2.0', capabilities, impl);
//!
//! // 3. Serve the RMCP client
//! const clientObj = await clientInfo.serve(transport);
//! const client = clientObj.inner;
//!
//! // 4. Interact with the server
//! const info = await client.peerInfo();
//! console.log('Connected to server:', info);
//! const tools = await client.listAllTools();
//! console.log('Available tools:', tools);
//! const result = await client.callTool('increment', { value: 1 });
//! console.log('Tool result:', result);
//! ```
//!
//! For more details, see the documentation for each module and struct.

pub mod model;
pub mod client;
pub mod service;
pub mod transport;

// Re-export capability types for WASM/TS bindings
pub use model::capabilities::{
    JsPromptsCapability,
    JsResourcesCapability,
    JsToolsCapability,
    JsRootsCapabilities,
    JsExperimentalCapabilities,
    JsClientCapabilities,
};

// Re-export model types
pub use model::{
    JsClientInfo,
    JsImplementation,
    ProtocolVersion,
};

// Re-export service types
pub use service::JsClient;

