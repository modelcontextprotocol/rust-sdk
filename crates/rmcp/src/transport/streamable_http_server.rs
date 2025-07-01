//! Streamable HTTP Server Transport Module
//!
//! This module provides streamable HTTP transport implementations for MCP.
//!
//! # Module Organization
//!
//! Framework-specific implementations are organized in submodules:
//! - `axum` - Contains the Axum-based streamable HTTP service implementation
//! - `actix_web` - Contains the actix-web-based streamable HTTP service implementation
//!
//! Each submodule exports a `StreamableHttpService` type with the same interface.
//!
//! For convenience, a type alias `StreamableHttpService` is provided at the module root that resolves to:
//! - `actix_web::StreamableHttpService` when the `actix-web` feature is enabled
//! - `axum::StreamableHttpService` when only the `axum` feature is enabled
//!
//! # Examples
//!
//! Using the convenience alias (recommended for most use cases):
//! ```ignore
//! use rmcp::transport::StreamableHttpService;
//! let service = StreamableHttpService::new(|| Ok(handler), session_manager, config);
//! ```
//!
//! Using framework-specific modules (when you need a specific implementation):
//! ```ignore
//! #[cfg(feature = "axum")]
//! use rmcp::transport::streamable_http_server::axum::StreamableHttpService;
//! #[cfg(feature = "axum")]
//! let service = StreamableHttpService::new(|| Ok(handler), session_manager, config);
//! ```

pub mod session;

use std::time::Duration;

/// Configuration for the streamable HTTP server
#[derive(Debug, Clone)]
pub struct StreamableHttpServerConfig {
    /// The ping message duration for SSE connections.
    pub sse_keep_alive: Option<Duration>,
    /// If true, the server will create a session for each request and keep it alive.
    pub stateful_mode: bool,
}

impl Default for StreamableHttpServerConfig {
    fn default() -> Self {
        Self {
            sse_keep_alive: Some(Duration::from_secs(15)),
            stateful_mode: true,
        }
    }
}

// Axum implementation
#[cfg(all(feature = "transport-streamable-http-server", feature = "axum"))]
#[cfg_attr(
    docsrs,
    doc(cfg(all(feature = "transport-streamable-http-server", feature = "axum")))
)]
pub mod axum;

// Actix-web implementation
#[cfg(all(feature = "transport-streamable-http-server", feature = "actix-web"))]
#[cfg_attr(
    docsrs,
    doc(cfg(all(feature = "transport-streamable-http-server", feature = "actix-web")))
)]
pub mod actix_web;

// Export the preferred implementation as StreamableHttpService (without generic parameters)
#[cfg(all(feature = "transport-streamable-http-server", feature = "actix-web"))]
pub use actix_web::StreamableHttpService;
#[cfg(all(
    feature = "transport-streamable-http-server",
    feature = "axum",
    not(feature = "actix-web")
))]
pub use axum::StreamableHttpService;
pub use session::{SessionId, SessionManager};
