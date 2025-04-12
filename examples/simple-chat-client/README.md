# Simple Chat Client

A simple chat client implementation using the Model Context Protocol (MCP) SDK. It just a example for developers to understand how to use the MCP SDK. This example use the easiest way to start a MCP server, and call the tool directly. No need embedding or complex third library or function call(because some models can't support function call).Just add tool in system prompt, and the client will call the tool automatically.


## Config
the config file is in `src/config.toml`. you can change the config to your own.Move the config file to `/etc/simple-chat-client/config.toml` for system-wide configuration.

## Usage

After configuring the config file, you can run the example:
```bash
cargo run --bin simple_chat
```

