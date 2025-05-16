pub mod model;
pub mod client;
pub mod service;
pub mod transport;

// Re-export capability types for WASM/TS bindings
pub use model::capabilities::{
    JsPromptsCapability,
    JsResourcesCapability,
    JsToolsCapability,
    JsRootsCapabilities,
    JsExperimentalCapabilities,
    JsClientCapabilities,
};

// Re-export model types
pub use model::{
    JsClientInfo,
    JsImplementation,
    ProtocolVersion,
};

// Re-export service types
pub use service::JsPeer;

