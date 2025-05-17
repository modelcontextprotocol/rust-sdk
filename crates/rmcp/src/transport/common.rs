#[cfg(any(
    feature = "transport-streamable-http-server",
    feature = "transport-sse-server"
))]
pub mod axum;

pub mod http_header;

#[cfg(feature = "reqwest")]
#[cfg_attr(docsrs, doc(cfg(feature = "reqwest")))]
mod reqwest;

#[cfg(feature = "client-side-sse")]
#[cfg_attr(docsrs, doc(cfg(feature = "client-side-sse")))]
pub mod client_side_sse;

#[cfg(feature = "auth")]
#[cfg_attr(docsrs, doc(cfg(feature = "auth")))]
pub mod auth;
