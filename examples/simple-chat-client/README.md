# Simple Chat Client

A simple chat client implementation using the Model Context Protocol (MCP) SDK. It just a example for developers to understand how to use the MCP SDK. This example use the easiest way to start a MCP server, and call the tool directly. No need embedding or complex third library or function call(because some models can't support function call).Just add tool in system prompt, and the client will call the tool automatically.


## Usage

After configuring the config file, you can run the example:
```bash
./simple_chat --help                                                       # show help info
./simple_chat config > config.toml                                         # output default config to file
./simple_chat --config my_config.toml chat                                 # start chat with specified config
./simple_chat --config my_config.toml --model gpt-4o-mini chat             # start chat with specified model
``` 

