<style>
.rustdoc-hidden { display: none; }
</style>

<div class="rustdoc-hidden">

# rmcp

[![Crates.io](https://img.shields.io/crates/v/rmcp.svg)](https://crates.io/crates/rmcp)
[![Documentation](https://docs.rs/rmcp/badge.svg)](https://docs.rs/rmcp)

</div>

The official Rust SDK for the [Model Context Protocol](https://modelcontextprotocol.io/specification/2025-11-25). Build MCP servers that expose tools, resources, and prompts to AI assistants — or build clients that connect to them.

For **getting started**, **usage guides**, and **full MCP feature documentation** (resources, prompts, sampling, roots, logging, completions, subscriptions, etc.), see the [main README](../../README.md).

## Feature Flags

| Feature | Description | Default |
|---------|-------------|---------|
| `server` | Server functionality and the tool system | ✅ |
| `client` | Client functionality | |
| `macros` | `#[tool]` / `#[prompt]` macros (re-exports [`rmcp-macros`](../rmcp-macros)) | ✅ |
| `schemars` | JSON Schema generation for tool definitions | |
| `auth` | OAuth 2.0 authentication support | |
| `elicitation` | Elicitation support | |

### Transport features

| Feature | Description |
|---------|-------------|
| `transport-io` | Server-side stdio transport |
| `transport-child-process` | Client-side stdio transport (spawns a child process) |
| `transport-async-rw` | Generic async read/write transport |
| `transport-streamable-http-client` | Streamable HTTP client (transport-agnostic) |
| `transport-streamable-http-client-reqwest` | Streamable HTTP client with default `reqwest` backend |
| `transport-streamable-http-server` | Streamable HTTP server transport |

### TLS backend options (for HTTP transports)

| Feature | Description |
|---------|-------------|
| `reqwest` | Uses rustls — pure Rust TLS (recommended default) |
| `reqwest-native-tls` | Uses platform-native TLS (OpenSSL / Secure Transport / SChannel) |
| `reqwest-tls-no-provider` | Uses rustls without a default crypto provider (bring your own) |

## Transports

The transport layer is pluggable. Two built-in pairs cover the most common cases:

| | Client | Server |
|:-:|:-:|:-:|
| **stdio** | [`TokioChildProcess`](crate::transport::TokioChildProcess) | [`stdio`](crate::transport::stdio) |
| **Streamable HTTP** | [`StreamableHttpClientTransport`](crate::transport::StreamableHttpClientTransport) | `StreamableHttpService` |

Any type that implements the [`Transport`](crate::transport::Transport) trait can be used. The [`IntoTransport`](crate::transport::IntoTransport) helper trait provides automatic conversions from:

1. `(Sink, Stream)` or a combined `Sink + Stream`
2. `(AsyncRead, AsyncWrite)` or a combined `AsyncRead + AsyncWrite`
3. A [`Worker`](crate::transport::worker::Worker) implementation
4. A [`Transport`](crate::transport::Transport) implementation directly

## License

This project is licensed under the terms specified in the repository's [LICENSE](../../LICENSE) file.
