import logging
import asyncio
from rmcp_python import PyClientInfo, PyClientCapabilities, PyImplementation, PyTransport, PySseTransport

logging.basicConfig(level=logging.INFO)

# The SSE endpoint for the MCP server
SSE_URL = "http://localhost:8000/sse"

async def main():
    # Create the SSE transport
    transport = await PySseTransport.start(SSE_URL)

    # Wrap the transport in PyTransport mimics the IntoTransport of Rust
    transport = PyTransport.from_sse(transport)
    # Initialize client info similar to the Rust examples
    client_info = PyClientInfo(
        protocol_version="2025-03-26",  # Use default
        capabilities=PyClientCapabilities(),
        client_info=PyImplementation(
            name="test python sse client",
            version="0.0.1",
        )
    )

    # Serve the client using the transport (mimics client_info.serve(transport) in Rust)
    client = await client_info.serve(transport)

    # Print server info
    server_info = client.peer_info()
    logging.info(f"Connected to server: {server_info}")

    # List available tools
    tools = await client.list_all_tools()
    logging.info(f"Available tools: {tools}")

    # Optionally, call a tool (e.g., get_value)
    result = await client.call_tool("increment", {})
    logging.info(f"Tool result: {result}")

if __name__ == "__main__":
    asyncio.run(main())
