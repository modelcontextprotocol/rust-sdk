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
// The calculator server declares only the tools capability, so unsupported
// request methods must fail rather than returning empty successful results.
async function expectMethodNotFound(name, operation) {
  try {
    const result = await operation();
    throw new Error(`${name} unexpectedly succeeded: ${JSON.stringify(result)}`);
  } catch (err) {
    if (err.code !== ErrorCode.MethodNotFound) {
      throw err;
    }
  }
  console.log(`${name} correctly rejected with method-not-found`);
}
await expectMethodNotFound("prompts/list", () => client.listPrompts());
await expectMethodNotFound("completion/complete", () =>
  client.complete({
    ref: { type: "ref/prompt", name: "unknown" },
    argument: { name: "input", value: "value" },
  }),
);
await client.close();
