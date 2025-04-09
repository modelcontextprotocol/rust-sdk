mod error;
pub use error::Error;

pub mod model;
pub use model::*;
#[cfg(feature = "macros")]
pub use paste;
#[cfg(feature = "macros")]
pub use rmcp_macros::tool;
