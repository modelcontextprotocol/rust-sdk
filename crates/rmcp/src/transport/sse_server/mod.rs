//! SSE Server Transport Module
//!
//! This module provides Server-Sent Events (SSE) transport implementations for MCP.
//!
//! # Module Organization
//!
//! Framework-specific implementations are organized in submodules:
//! - `axum` - Contains the Axum-based SSE server implementation
//! - `actix_web` - Contains the actix-web-based SSE server implementation
//!
//! Each submodule exports a `SseServer` type with the same interface.
//!
//! For convenience, a type alias `SseServer` is provided at the module root that resolves to:
//! - `actix_web::SseServer` when the `actix-web` feature is enabled
//! - `axum::SseServer` when only the `axum` feature is enabled
//!
//! # Examples
//!
//! Using the convenience alias (recommended for most use cases):
//! ```ignore
//! use rmcp::transport::SseServer;
//! let server = SseServer::serve("127.0.0.1:8080".parse()?).await?;
//! ```
//!
//! Using framework-specific modules (when you need a specific implementation):
//! ```ignore
//! #[cfg(feature = "axum")]
//! use rmcp::transport::sse_server::axum::SseServer;
//! #[cfg(feature = "axum")]
//! let server = SseServer::serve("127.0.0.1:8080".parse()?).await?;
//! ```

#[cfg(feature = "transport-sse-server")]
pub mod common;

// Axum implementation
#[cfg(all(feature = "transport-sse-server", feature = "axum"))]
pub mod axum;

// Actix-web implementation
#[cfg(all(feature = "transport-sse-server", feature = "actix-web"))]
pub mod actix_web;

// Convenience type alias
#[cfg(all(feature = "transport-sse-server", feature = "actix-web"))]
pub use actix_web::SseServer;
#[cfg(all(
    feature = "transport-sse-server",
    feature = "axum",
    not(feature = "actix-web")
))]
pub use axum::SseServer;
// Re-export common types when transport-sse-server is enabled
#[cfg(feature = "transport-sse-server")]
pub use common::{DEFAULT_AUTO_PING_INTERVAL, SessionId, SseServerConfig, session_id};
