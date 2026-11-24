//! MCP Events Beta for protocol 2026-07-28 and later.
//!
//! Install an `EventsServer` in `ServerHandler::events`. Use `EventsClient`
//! with a peer that advertised Events in discovery.
//! Persist only checkpoints whose events you have processed.
//!
//! ```
//! # #[cfg(feature = "server")]
//! # fn main() -> Result<(), rmcp::events::wire::EventsError> {
//! use rmcp::{ServerHandler, events::{
//!     server::{CheckedBatch, Descriptor, EventsServer},
//!     wire::DeliveryMode,
//! }, model::JsonObject};
//!
//! struct App(EventsServer);
//! impl ServerHandler for App {
//!     fn events(&self) -> Option<&EventsServer> { Some(&self.0) }
//! }
//! let events = EventsServer::new();
//! events.register_check(
//!     Descriptor::typed::<JsonObject, JsonObject>("demo.tick", "Demo tick", [DeliveryMode::Poll]),
//!     |_, _, _| async { Ok(CheckedBatch::new("head")) },
//! )?;
//! let _app = App(events);
//! # Ok(())
//! # }
//! # #[cfg(not(feature = "server"))]
//! # fn main() {}
//! ```

#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "server")]
pub mod server;
#[cfg(feature = "events-webhook-receiver")]
pub mod webhook;
pub mod wire;

#[cfg(any(feature = "server", feature = "client"))]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct StreamParams {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    arguments: Option<crate::model::JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_age_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    heartbeat_ms: Option<u64>,
}

pub(crate) const EXTENSION: &str = "io.modelcontextprotocol/events";
pub(crate) fn advertises_list_changes(capabilities: &crate::model::ServerCapabilities) -> bool {
    capabilities
        .extensions
        .as_ref()
        .and_then(|extensions| extensions.get(EXTENSION))
        .and_then(|settings| settings.get("listChanged"))
        .and_then(serde_json::Value::as_bool)
        == Some(true)
}

#[cfg(any(feature = "server", feature = "client"))]
mod methods {
    pub(crate) const LIST: &str = "events/list";
    pub(crate) const POLL: &str = "events/poll";
    pub(crate) const STREAM: &str = "events/stream";
    pub(crate) const ACTIVE: &str = "notifications/events/active";
    pub(crate) const EVENT: &str = "notifications/events/event";
    pub(crate) const HEARTBEAT: &str = "notifications/events/heartbeat";
    pub(crate) const ERROR: &str = "notifications/events/error";
    pub(crate) const TERMINATED: &str = "notifications/events/terminated";
    pub(crate) const LIST_CHANGED: &str = "notifications/events/list_changed";
}
#[cfg(any(feature = "server", feature = "client"))]
pub(crate) use methods::*;

#[cfg(feature = "server")]
pub(crate) fn named_method(method: &str) -> bool {
    matches!(
        method,
        POLL | STREAM | "events/subscribe" | "events/unsubscribe"
    )
}
