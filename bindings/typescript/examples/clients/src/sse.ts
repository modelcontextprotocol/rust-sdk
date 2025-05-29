// TypeScript SSE client example for rmcp_typescript
// Closely mirrors the structure and flow of sse.rs from the core Rust SDK

import { JsTransport, JsSseTransport, JsClientInfo, JsImplementation, JsClientCapabilities, JsExperimentalCapabilities, JsRootsCapabilities, JsClient } from 'rmcp-typescript';

// TODO: Replace 'any' with proper types from WASM/SDK when available
// type ClientCapabilities = any;
// type CallToolRequestParam = { name: string; arguments?: object };

async function main() {
  // Step 1: Initialize logging (console-based for now)
  console.info('Starting TypeScript SSE client example');

  // Step 2: Create transport
  const sseEndpoint = 'http://localhost:8000/sse';
  const sseTransport = await JsSseTransport.start(sseEndpoint);
  console.log('sseTransport:', sseTransport);
  const transport = JsTransport.fromSse(sseTransport);
  console.log('transport:', transport.kind);

  // Step 3: Define client info (mirror Rust structure)
  const experimental = JsExperimentalCapabilities.new({});
  const roots = new JsRootsCapabilities();
  const sampling = null;
  console.log('JsClientCapabilities.new args:', { experimental, roots, sampling });
  const clientInfo = new JsClientInfo(
    '2024-11-05',
    new JsClientCapabilities(experimental, roots, sampling),
    new JsImplementation('typescript-sse-client', '0.0.1')
  );

  try {
    // Step 4: Start the client and get peer info
    const clientObj = await clientInfo.serve(transport);
    const client = clientObj.inner as JsClient;
    const serverInfo = client.peerInfo();
    console.info('Connected to server:', serverInfo);

    // Step 5: List available tools
    const tools = await client.listAllTools();
    console.info('Available tools:', tools);

    // Step 6: Call a tool (example)
    
    const result = await client.callTool("increment", {});
    console.info('Tool result:', result);


    // Keep connection open for demonstration
    setTimeout(() => {
      console.info('Connection closed.');
    }, 10000);

  } catch (e) {
    console.error('Client error:', e);
  }
}

main();
