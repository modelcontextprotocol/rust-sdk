#![doc = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md"))]
mod error;
pub use error::Error;

/// Basic data types in MCP specification
pub mod model;
#[cfg(any(feature = "client", feature = "server"))]
pub mod service;
#[cfg(any(feature = "client", feature = "server"))]
pub use service::{Peer, Service, ServiceError, ServiceExt};
#[cfg(feature = "client")]
pub use service::{RoleClient, serve_client};
#[cfg(feature = "server")]
pub use service::{RoleServer, serve_server};

#[cfg(feature = "client")]
pub use handler::client::ClientHandler;
#[cfg(feature = "server")]
pub use handler::server::ServerHandler;

pub mod handler;
pub mod transport;

#[cfg(all(feature = "macros", feature = "server"))]
pub use rmcp_macros::tool;

// re-export
#[cfg(all(feature = "macros", feature = "server"))]
pub use paste::paste;
#[cfg(all(feature = "macros", feature = "server"))]
pub use schemars;
#[cfg(feature = "macros")]
pub use serde;
#[cfg(feature = "macros")]
pub use serde_json;
