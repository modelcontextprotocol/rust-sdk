from mcp import ClientSession, StdioServerParameters, types
from mcp.client.sse import sse_client



async def run():
    async with sse_client("http://localhost:8000/sse") as (read, write):
        async with ClientSession(
            read, write
        ) as session:
            # Initialize the connection
            await session.initialize()

            # List available prompts
            prompts = await session.list_prompts()
            print(prompts)
            # List available resources
            resources = await session.list_resources()
            print(resources)

            # List available tools
            tools = await session.list_tools()
            print(tools)

if __name__ == "__main__":
    import asyncio

    asyncio.run(run())