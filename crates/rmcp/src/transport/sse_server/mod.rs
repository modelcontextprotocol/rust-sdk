#[cfg(feature = "transport-sse-server")]
pub mod common;

// When only axum is enabled
#[cfg(all(feature = "transport-sse-server", feature = "axum", not(feature = "actix-web")))]
mod axum_impl;

#[cfg(all(feature = "transport-sse-server", feature = "axum", not(feature = "actix-web")))]
pub use axum_impl::*;

// When actix-web is enabled (with or without axum)
#[cfg(all(feature = "transport-sse-server", feature = "actix-web"))]
mod actix_impl;

#[cfg(all(feature = "transport-sse-server", feature = "actix-web"))]
pub use actix_impl::*;

// When both are enabled, also provide axum implementation under different name
#[cfg(all(feature = "transport-sse-server", feature = "axum", feature = "actix-web"))]
pub mod axum_impl;

#[cfg(all(feature = "transport-sse-server", feature = "axum", feature = "actix-web"))]
pub use axum_impl::SseServer as AxumSseServer;

// Re-export common types when transport-sse-server is enabled
#[cfg(feature = "transport-sse-server")]
pub use common::{SseServerConfig, SessionId, session_id, DEFAULT_AUTO_PING_INTERVAL};