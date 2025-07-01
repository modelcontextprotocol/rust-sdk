//! Streamable HTTP Server Transport Module
//!
//! This module provides streamable HTTP transport implementations for MCP.
//!
//! # Type Export Strategy
//!
//! This module exports framework-specific implementations with explicit names:
//! - `AxumStreamableHttpService` - The Axum-based streamable HTTP service implementation
//! - `ActixStreamableHttpService` - The actix-web-based streamable HTTP service implementation
//!
//! For convenience, a type alias `StreamableHttpService` is provided that resolves to:
//! - `ActixStreamableHttpService` when the `actix-web` feature is enabled
//! - `AxumStreamableHttpService` when only the `axum` feature is enabled
//!
//! # Examples
//!
//! Using the convenience alias (recommended for most use cases):
//! ```ignore
//! use rmcp::transport::StreamableHttpService;
//! let service = StreamableHttpService::new(|| Ok(handler), session_manager, config);
//! ```
//!
//! Using explicit types (when you need a specific implementation):
//! ```ignore
//! #[cfg(feature = "axum")]
//! use rmcp::transport::AxumStreamableHttpService;
//! #[cfg(feature = "axum")]
//! let service = AxumStreamableHttpService::new(|| Ok(handler), session_manager, config);
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
#[cfg_attr(docsrs, doc(cfg(all(feature = "transport-streamable-http-server", feature = "axum"))))]
pub mod tower;

#[cfg(all(feature = "transport-streamable-http-server", feature = "axum"))]
pub use tower::StreamableHttpService as AxumStreamableHttpService;

// Actix-web implementation
#[cfg(all(feature = "transport-streamable-http-server", feature = "actix-web"))]
#[cfg_attr(docsrs, doc(cfg(all(feature = "transport-streamable-http-server", feature = "actix-web"))))]
pub mod actix_impl;

#[cfg(all(feature = "transport-streamable-http-server", feature = "actix-web"))]
pub use actix_impl::StreamableHttpService as ActixStreamableHttpService;

// Export the preferred implementation as StreamableHttpService (without generic parameters)
#[cfg(all(feature = "transport-streamable-http-server", feature = "actix-web"))]
pub use actix_impl::StreamableHttpService;

#[cfg(all(feature = "transport-streamable-http-server", feature = "axum", not(feature = "actix-web")))]
pub use tower::StreamableHttpService;

pub use session::{SessionId, SessionManager};
