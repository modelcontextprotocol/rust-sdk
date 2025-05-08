use std::time::Duration;

use futures::stream::BoxStream;
use sse_stream::{Error as SseError, Sse};

pub type BoxedSseResponse = BoxStream<'static, Result<Sse, SseError>>;

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct SseRetryConfig {
    pub max_times: Option<usize>,
    pub min_duration: Duration,
}

impl SseRetryConfig {
    pub const DEFAULT_MIN_DURATION: Duration = Duration::from_millis(1000);
}

impl Default for SseRetryConfig {
    fn default() -> Self {
        Self {
            max_times: None,
            min_duration: Self::DEFAULT_MIN_DURATION,
        }
    }
}
