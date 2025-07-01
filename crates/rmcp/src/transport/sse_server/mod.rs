//! SSE Server Transport Module
//!
//! This module provides Server-Sent Events (SSE) transport implementations for MCP.
//!
//! # Type Export Strategy
//!
//! This module exports framework-specific implementations with explicit names:
//! - `AxumSseServer` - The Axum-based SSE server implementation
//! - `ActixSseServer` - The actix-web-based SSE server implementation
//!
//! For convenience, a type alias `SseServer` is provided that resolves to:
//! - `ActixSseServer` when the `actix-web` feature is enabled
//! - `AxumSseServer` when only the `axum` feature is enabled
//!
//! # Examples
//!
//! Using the convenience alias (recommended for most use cases):
//! ```ignore
//! use rmcp::transport::SseServer;
//! let server = SseServer::serve("127.0.0.1:8080".parse()?).await?;
//! ```
//!
//! Using explicit types (when you need a specific implementation):
//! ```ignore
//! #[cfg(feature = "axum")]
//! use rmcp::transport::AxumSseServer;
//! #[cfg(feature = "axum")]
//! let server = AxumSseServer::serve("127.0.0.1:8080".parse()?).await?;
//! ```

#[cfg(feature = "transport-sse-server")]
pub mod common;

// Axum implementation
#[cfg(all(feature = "transport-sse-server", feature = "axum"))]
mod axum_impl;

#[cfg(all(feature = "transport-sse-server", feature = "axum"))]
pub use axum_impl::SseServer as AxumSseServer;

// Actix-web implementation
#[cfg(all(feature = "transport-sse-server", feature = "actix-web"))]
mod actix_impl;

#[cfg(all(feature = "transport-sse-server", feature = "actix-web"))]
pub use actix_impl::SseServer as ActixSseServer;

// Convenience type alias
#[cfg(all(feature = "transport-sse-server", feature = "actix-web"))]
pub type SseServer = ActixSseServer;

#[cfg(all(feature = "transport-sse-server", feature = "axum", not(feature = "actix-web")))]
pub type SseServer = AxumSseServer;

// Re-export common types when transport-sse-server is enabled
#[cfg(feature = "transport-sse-server")]
pub use common::{SseServerConfig, SessionId, session_id, DEFAULT_AUTO_PING_INTERVAL};