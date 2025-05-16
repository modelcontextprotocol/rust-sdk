// TypeScript SSE client example for rmcp_typescript
// Closely mirrors the structure and flow of sse.rs from the core Rust SDK

import { JsTransport, JsTransportEnum, JsClientInfo, JsImplementation } from '../../index';

// TODO: Replace 'any' with proper types from WASM/SDK when available
// type ClientCapabilities = any;
// type CallToolRequestParam = { name: string; arguments?: object };

async function main() {
  // Step 1: Initialize logging (console-based for now)
  console.info('Starting TypeScript SSE client example');

  // Step 2: Create transport
  const sseEndpoint = 'http://localhost:8000/sse';
  const transport = new JsTransport(JsTransportEnum.SSE, sseEndpoint);

  // Step 3: Define client info (mirror Rust structure)
  const clientInfo: JsClientInfo = {
    protocolVersion: '2024-11-05', // TODO: Use ProtocolVersion.latest() if available
    capabilities: {}, // TODO: Use proper ClientCapabilities
    clientInfo: {
      name: 'typescript-sse-client',
      version: '0.0.1',
    },
  };

  try {
    // Step 4: Connect and serve (stub for now)
    transport.start(
      (data: string) => {
        console.log('Received SSE message:', data);
        // TODO: Parse and handle protocol messages here (initialize, tool list, etc.)
      },
      (err: Event) => {
        console.error('SSE error:', err);
      }
    );

    // TODO: Replace with real async/await protocol flow when WASM/SDK methods are available
    // Example (pseudo-code):
    /*
    const client = await clientInfo.serve(transport);
    const serverInfo = client.peerInfo();
    console.info('Connected to server:', serverInfo);

    const tools = await client.listTools({});
    console.info('Available tools:', tools);

    const toolResult = await client.callTool({
      name: 'increment',
      arguments: {},
    });
    console.info('Tool result:', toolResult);

    await client.cancel();
    */

    // For now, keep connection open for demonstration
    setTimeout(() => {
      transport.close();
      console.info('Connection closed.');
    }, 10000);

  } catch (e) {
    console.error('Client error:', e);
  }
}

main();
