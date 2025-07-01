use std::{net::SocketAddr, sync::Arc, time::Duration};

use tokio_util::sync::CancellationToken;

pub type SessionId = Arc<str>;

pub fn session_id() -> SessionId {
    uuid::Uuid::new_v4().to_string().into()
}

#[derive(Debug, Clone)]
pub struct SseServerConfig {
    pub bind: SocketAddr,
    pub sse_path: String,
    pub post_path: String,
    pub ct: CancellationToken,
    pub sse_keep_alive: Option<Duration>,
}

pub const DEFAULT_AUTO_PING_INTERVAL: Duration = Duration::from_secs(15);
