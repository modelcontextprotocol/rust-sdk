// TypeScript test for rmcp_typescript wasm binding
import * as wasm from '../dist/rmcp_typescript/index.js';

// Test ProtocolVersion class
const pv = new wasm.ProtocolVersion("2.0");
if (pv.value() !== "2.0") throw new Error('ProtocolVersion.value() failed');
if (pv.toString() !== "2.0") throw new Error('ProtocolVersion.toString() failed');

// Test ProtocolVersion.latest static method
const latest = wasm.ProtocolVersion.latest();
if (latest.value() !== "2.0") throw new Error('ProtocolVersion.latest() failed');

// Test json_rpc_version_2_0 function
if (wasm.json_rpc_version_2_0() !== "2.0") throw new Error('json_rpc_version_2_0() failed');

console.log('All ProtocolVersion and constant tests passed!');
