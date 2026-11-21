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
| `auth` | OAuth 2.0 authentication support | |
| `auth-enterprise-managed` | EMA/XAA refresh-token and ID-JAG exchanges for registered public and confidential clients (includes `auth`) | |
| `elicitation` | Elicitation support | |
| `skills-fs` | Serve SEP-2640 skills from a directory tree (`SkillCatalog`) | |
| `tool-search` | Dependency-free BM25 tool search and optional server discovery wrapper | |

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

## Tool search

Enable `tool-search` for a standalone, lazily built BM25 Okapi index:

```rust,ignore
use rmcp::tool_search::ToolSearchIndex;

let mut index = ToolSearchIndex::new(tools);
let hits = index.search("create a github issue", 5);
// After an upstream tools/list_changed notification, fetch the new catalog:
index.replace_tools(updated_tools);
```

Names, titles, descriptions, and input-schema parameter names/descriptions are
searched case-insensitively. Use `with_parameter_indexing(tools, false)` to omit
parameters. Hits include full `Tool` definitions and scores; unmatched queries
return no hits.

With `server` enabled, wrap your existing `ServerHandler` (including handlers
using `#[tool_handler]` / `ToolRouter`):

```rust,ignore
use rmcp::tool_search::{ToolSearchConfig, ToolSearchServer};

let mut config = ToolSearchConfig::default();
config.max_results = 5;
config.always_visible = vec!["health_check".into()];
let server = ToolSearchServer::with_config(my_handler, config)?;
// Serve `server` as usual.
```

`tools/list` exposes `search_tools` and `call_tool`, plus `always_visible` tools
which are excluded from search. Search arguments are
`{"query": "create an issue", "max_results": 3}`; the optional limit defaults to
the configured maximum (five by default) and cannot exceed it. Search returns
`{"tools": [...]}` as both structured and text content, including input schemas.
Dispatch arguments are `{"name": "create_issue", "arguments": {...}}`.
Original tools remain directly callable, and dispatch preserves request metadata
and retry state. Search controls discovery, **not authorization**: underlying
handlers must still enforce access checks.

Use `search_tool_name` / `call_tool_name` to avoid catalog collisions. Invalid
settings are rejected at construction, and catalog collisions fail listing/search.
The wrapper fetches every catalog page on each listing/search and lazily rebuilds
its index only when definitions change, so dynamic or request-specific catalogs
do not require a notification invalidation hook. Other handler methods and
notifications are delegated unchanged. Regex and embedding search are not included.

## License

This project is licensed under the terms specified in the repository's [LICENSE](../../LICENSE) file.
