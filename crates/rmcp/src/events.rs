//! MCP Events Beta for protocol 2026-07-28 and later.
//!
//! Compiling this module does not opt either peer into Events.
//!
//! Install an SDK runtime in `ServerHandler::events` to take over
//! Events dispatch. Enable the `server` feature for this example:
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
//!
//! On the client, use discovery startup and call `EventsClient::list_page`
//! and `EventsClient::poll`. Persist only the cursor of a consumed page.
//! Poll result payloads still appear in DEBUG response logs because `CustomResult` carries no method name.

#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "server")]
pub mod server;
pub mod wire;

pub(crate) const EXTENSION: &str = "io.modelcontextprotocol/events";
#[cfg(any(feature = "server", feature = "client"))]
pub(crate) const LIST_CHANGED: &str = "notifications/events/list_changed";

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
const LIST: &str = "events/list";
#[cfg(any(feature = "server", feature = "client"))]
const POLL: &str = "events/poll";

#[cfg(any(
    feature = "server",
    all(feature = "client", feature = "transport-streamable-http-client")
))]
pub(crate) fn named_method(method: &str) -> bool {
    matches!(
        method,
        POLL | "events/stream" | "events/subscribe" | "events/unsubscribe"
    )
}

#[cfg(feature = "server")]
pub(crate) fn clear_advertisement(capabilities: &mut crate::model::ServerCapabilities) {
    if let Some(extensions) = &mut capabilities.extensions
        && extensions.remove(EXTENSION).is_some()
        && extensions.is_empty()
    {
        capabilities.extensions = None;
    }
}
