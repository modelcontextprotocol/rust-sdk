<style>
.rustdoc-hidden { display: none; }
</style>

<div class="rustdoc-hidden">

# rmcp

[![Crates.io](https://img.shields.io/crates/v/rmcp.svg)](https://crates.io/crates/rmcp)
[![Documentation](https://docs.rs/rmcp/badge.svg)](https://docs.rs/rmcp)

</div>

The official Rust SDK for the [Model Context Protocol](https://modelcontextprotocol.io/specification/2026-07-28). Build MCP servers that expose tools, resources, and prompts to AI assistants — or build clients that connect to them.

For **getting started**, **usage guides**, and **full MCP feature documentation** (resources, prompts, sampling, roots, logging, completions, subscriptions, etc.), see the [main README](../../README.md).

## Feature Flags

| Feature | Description | Default |
|---------|-------------|---------|
| `server` | Server functionality and the tool system | ✅ |
| `client` | Client functionality | |
| `macros` | `#[tool]` / `#[prompt]` macros (re-exports [`rmcp-macros`](../rmcp-macros)) | ✅ |
| `schemars` | JSON Schema generation for tool definitions | |
| `chrono` | `chrono::DateTime` arguments for `Annotations::for_resource` and `with_timestamp`, and schemars' `chrono04` support | ✅ |
| `auth` | OAuth 2.0 authentication support | |
| `auth-enterprise-managed` | EMA/XAA refresh-token and ID-JAG exchanges for registered public and confidential clients (includes `auth`) | |
| `elicitation` | Elicitation support | |
| `skills-fs` | Serve SEP-2640 skills from a directory tree (`SkillCatalog`) | |

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

For enterprise-managed authorization, enable `auth-enterprise-managed` and a TLS
backend such as `reqwest`. See the [EMA/XAA guide](../../docs/OAUTH_SUPPORT.md#enterprise-managed-authorization-emaxaa)
for client authentication and an MCP connection example.

### Building without chrono

The `chrono` feature is on by default. To leave chrono out (for example because of
[RUSTSEC-2026-0543](https://rustsec.org/advisories/RUSTSEC-2026-0543.html)), turn off the
default features and list the ones you use:

```toml
[dependencies]
rmcp = { version = "3", default-features = false, features = ["base64", "macros", "server", "transport-io"] }
```

Without `chrono`, `Annotations::for_resource` and `Annotations::with_timestamp` are not
available; `Annotations::with_timestamp_now` and the public `last_modified` field (an RFC 3339
string) still work. The `auth` features still pull chrono in through `oauth2` 5, and
`cargo tree -i chrono` shows whether anything else does. `chrono` is planned to leave the
default features in the next major release.

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
