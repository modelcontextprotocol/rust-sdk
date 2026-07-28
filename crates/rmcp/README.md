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

### Use-case bundles

Start here. Each bundle is one flag for one scenario.

| Feature | Equivalent to |
|---------|---------------|
| `server-stdio` | `server` + `macros` + `transport-stdio` |
| `server-http` | `server` + `macros` + `transport-streamable-http-server` |
| `client-stdio` | `client` + `transport-child-process` |
| `client-http` | `client` + `transport-streamable-http-client-reqwest` + `tls-rustls` |

```sh
cargo add rmcp --features server-stdio
```

The sections below are for composing a build the bundles don't cover.

### Roles

| Feature | Description | Default |
|---------|-------------|---------|
| `server` | Server functionality and the tool system | ✅ |
| `client` | Client functionality | |
| `macros` | `#[tool]` / `#[prompt]` macros (re-exports [`rmcp-macros`](../rmcp-macros)) | ✅ |

### Protocol capabilities

Tools, prompts, resources, sampling, elicitation, completion, subscriptions, tasks, and response caching need no feature flag. Only capabilities carrying a heavy dependency are opt-in:

| Feature | Description |
|---------|-------------|
| `auth` | OAuth 2.0 client support |
| `auth-client-credentials-jwt` | `private_key_jwt` client authentication |
| `request-state` | SEP-2322 `requestState` integrity sealing (`RequestStateCodec`) |

### Transports

| Feature | Description |
|---------|-------------|
| `transport-stdio` | Server-side stdio transport |
| `transport-child-process` | Client-side stdio transport (spawns a child process) |
| `transport-async-rw` | Generic async read/write transport |
| `transport-worker` | Build a transport from a [`Worker`](crate::transport::worker::Worker) implementation |
| `transport-streamable-http-server` | Streamable HTTP server transport |
| `transport-streamable-http-client` | Streamable HTTP client, bring your own HTTP stack |
| `transport-streamable-http-client-reqwest` | …with the `reqwest` backend |
| `transport-streamable-http-client-unix-socket` | …over a Unix domain socket |

### TLS backends

Pick one when using a `reqwest`-backed feature (`transport-streamable-http-client-reqwest` or `auth`):

| Feature | Description |
|---------|-------------|
| `tls-rustls` | rustls — pure Rust TLS (recommended) |
| `tls-native` | Platform-native TLS (OpenSSL / Secure Transport / SChannel) |
| `tls-no-provider` | rustls without a default crypto provider (bring your own) |

> **Picking none of them still compiles.** `reqwest` is then built without a TLS
> backend, so `http://` works and every `https://` request fails at runtime.
> `client-http` bundles `tls-rustls` for you; the bare combination stays
> available for plain-`http://` deployments that want it.

### Ecosystem integrations

| Feature | Description |
|---------|-------------|
| `schemars` | JSON Schema generation for tool definitions (implied by `server`) |
| `tower` | `tower::Service` integration (implied by the HTTP server transport) |

### Single-threaded and WASM

| Feature | Description |
|---------|-------------|
| `unsync` | Drops the `Send` bounds on futures crate-wide |

> **`unsync` is not additive.** Cargo unifies features across the whole
> dependency graph, so any crate enabling it changes the trait bounds for every
> other user of `rmcp` in the same build. Enable it only from a leaf binary.

### Renamed in v3.0

Old names keep working as aliases and are removed in v4.0.

| v2 | v3 |
|----|----|
| `transport-io` | `transport-stdio` |
| `which-command` | folded into `transport-child-process` |
| `reqwest` | `tls-rustls` |
| `reqwest-native-tls` | `tls-native` |
| `reqwest-tls-no-provider` | `tls-no-provider` |
| `local` | `unsync` |
| `elicitation` | no-op — the capability is always available |
| `base64` | no-op — image content helpers are always available |

`uuid`, `client-side-sse`, `server-side-http`, and `transport-streamable-http-server-session` were implementation details and are gone. The transport features that used to compose them still do, so nothing needs to replace them.

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
