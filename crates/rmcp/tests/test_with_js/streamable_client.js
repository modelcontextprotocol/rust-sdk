import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";
import { ErrorCode } from "@modelcontextprotocol/sdk/types.js";

const transport = new StreamableHTTPClientTransport(new URL(`http://127.0.0.1:8001/mcp/`));

const client = new Client(
  {
    name: "example-client",
    version: "1.0.0"
  },
  {
    capabilities: {
      prompts: {},
      resources: {},
      tools: {}
    }
  }
);
await client.connect(transport);
const tools = await client.listTools();
console.log(tools);
const resources = await client.listResources();
console.log(resources);
const templates = await client.listResourceTemplates();
console.log(templates);
// The calculator server declares only the tools capability, so the server
// must answer prompts/list with method-not-found instead of an empty result.
try {
  const prompts = await client.listPrompts();
  console.error(`listPrompts unexpectedly succeeded: ${JSON.stringify(prompts)}`);
  process.exit(1);
} catch (err) {
  if (err.code !== ErrorCode.MethodNotFound) {
    console.error(`listPrompts failed with an unexpected error: ${err}`);
    process.exit(1);
  }
  console.log("listPrompts correctly rejected with method-not-found");
}
await client.close();
