use std::{sync::Arc, time::Duration};

pub type SessionId = Arc<str>;

pub fn session_id() -> SessionId {
    uuid::Uuid::new_v4().to_string().into()
}

pub const DEFAULT_AUTO_PING_INTERVAL: Duration = Duration::from_secs(15);
