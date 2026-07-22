use std::{
    pin::Pin,
    sync::Arc,
    task::{Poll, ready},
    time::Duration,
};

use bytes::Bytes;
use futures::{Stream, StreamExt, stream::BoxStream};
use sse_stream::{Error as SseError, Sse, SseStream};
use thiserror::Error;

use crate::model::ServerJsonRpcMessage;

pub type BoxedSseResponse = BoxStream<'static, Result<Sse, SseError>>;

/// Maximum raw size of one SSE event accepted from a remote server.
pub(crate) const DEFAULT_MAX_SSE_EVENT_SIZE: usize = 16 * 1024 * 1024;

#[derive(Debug, Error)]
enum BoundedSseStreamError {
    #[error(transparent)]
    Source(Box<dyn std::error::Error + Send + Sync>),
    #[error("SSE event exceeded the maximum size of {max_size} bytes")]
    EventTooLarge { max_size: usize },
}

#[derive(Debug)]
struct SseEventSizeLimiter {
    max_size: usize,
    retained_size: usize,
    line_size: usize,
    line_is_comment: bool,
    previous_was_cr: bool,
}

impl SseEventSizeLimiter {
    fn new(max_size: usize) -> Self {
        Self {
            max_size,
            retained_size: 0,
            line_size: 0,
            line_is_comment: false,
            previous_was_cr: false,
        }
    }

    fn observe(&mut self, chunk: &[u8]) -> Result<(), ()> {
        for &byte in chunk {
            if self.previous_was_cr {
                self.previous_was_cr = false;
                if byte == b'\n' {
                    continue;
                }
            }

            match byte {
                b'\r' => {
                    self.finish_line()?;
                    self.previous_was_cr = true;
                }
                b'\n' => self.finish_line()?,
                _ => {
                    if self.line_size == 0 {
                        self.line_is_comment = byte == b':';
                    }
                    self.line_size = self.line_size.saturating_add(1);
                    self.check_limit()?;
                }
            }
        }
        Ok(())
    }

    fn finish_line(&mut self) -> Result<(), ()> {
        if self.line_size == 0 {
            self.retained_size = 0;
        } else if !self.line_is_comment {
            // The SSE parser inserts a newline when joining multiple data fields.
            self.retained_size = self
                .retained_size
                .saturating_add(self.line_size)
                .saturating_add(1);
        }
        self.line_size = 0;
        self.line_is_comment = false;
        self.check_limit()
    }

    fn check_limit(&self) -> Result<(), ()> {
        if self.retained_size.saturating_add(self.line_size) > self.max_size {
            Err(())
        } else {
            Ok(())
        }
    }
}

pin_project_lite::pin_project! {
    struct BoundedSseByteStream<S> {
        #[pin]
        inner: S,
        limiter: SseEventSizeLimiter,
        failed: bool,
    }
}

impl<S, E> Stream for BoundedSseByteStream<S>
where
    S: Stream<Item = Result<Bytes, E>>,
    E: std::error::Error + Send + Sync + 'static,
{
    type Item = Result<Bytes, BoundedSseStreamError>;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        if *this.failed {
            return Poll::Ready(None);
        }

        match ready!(this.inner.as_mut().poll_next(cx)) {
            Some(Ok(chunk)) => {
                if this.limiter.observe(&chunk).is_err() {
                    *this.failed = true;
                    Poll::Ready(Some(Err(BoundedSseStreamError::EventTooLarge {
                        max_size: this.limiter.max_size,
                    })))
                } else {
                    Poll::Ready(Some(Ok(chunk)))
                }
            }
            Some(Err(error)) => {
                *this.failed = true;
                Poll::Ready(Some(Err(BoundedSseStreamError::Source(Box::new(error)))))
            }
            None => Poll::Ready(None),
        }
    }
}

pub(crate) fn bounded_sse_stream<S, E>(stream: S, max_event_size: usize) -> BoxedSseResponse
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    let stream = BoundedSseByteStream {
        inner: stream,
        limiter: SseEventSizeLimiter::new(max_event_size),
        failed: false,
    };
    SseStream::from_bytes_stream(stream).boxed()
}

fn is_event_too_large_error(error: &SseError) -> bool {
    matches!(
        error,
        SseError::Body(error)
            if matches!(
                error.downcast_ref::<BoundedSseStreamError>(),
                Some(BoundedSseStreamError::EventTooLarge { .. })
            )
    )
}

pub trait SseRetryPolicy: std::fmt::Debug + Send + Sync {
    fn retry(&self, current_times: usize) -> Option<Duration>;
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct FixedInterval {
    pub max_times: Option<usize>,
    pub duration: Duration,
}

impl SseRetryPolicy for FixedInterval {
    fn retry(&self, current_times: usize) -> Option<Duration> {
        if let Some(max_times) = self.max_times {
            if current_times >= max_times {
                return None;
            }
        }
        Some(self.duration)
    }
}

impl FixedInterval {
    pub const DEFAULT_MIN_DURATION: Duration = Duration::from_millis(1000);
}

impl Default for FixedInterval {
    fn default() -> Self {
        Self {
            max_times: None,
            duration: Self::DEFAULT_MIN_DURATION,
        }
    }
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ExponentialBackoff {
    pub max_times: Option<usize>,
    pub base_duration: Duration,
}

impl ExponentialBackoff {
    pub const DEFAULT_DURATION: Duration = Duration::from_millis(1000);
}

impl Default for ExponentialBackoff {
    fn default() -> Self {
        Self {
            max_times: None,
            base_duration: Self::DEFAULT_DURATION,
        }
    }
}

impl SseRetryPolicy for ExponentialBackoff {
    fn retry(&self, current_times: usize) -> Option<Duration> {
        if let Some(max_times) = self.max_times {
            if current_times >= max_times {
                return None;
            }
        }
        Some(self.base_duration * (2u32.pow(current_times as u32)))
    }
}

#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct NeverRetry;

impl SseRetryPolicy for NeverRetry {
    fn retry(&self, _current_times: usize) -> Option<Duration> {
        None
    }
}

#[derive(Debug, Default)]
pub struct NeverReconnect<E> {
    error: Option<E>,
}

impl<E: std::error::Error + Send> SseStreamReconnect for NeverReconnect<E> {
    type Error = E;
    type Future = futures::future::Ready<Result<BoxedSseResponse, Self::Error>>;
    fn retry_connection(&mut self, _last_event_id: Option<&str>) -> Self::Future {
        futures::future::ready(Err(self.error.take().expect("should not be called again")))
    }
}

/// Abstraction for SSE reconnection logic. Implementors can hook into
/// [`handle_control_event`](Self::handle_control_event) to consume control
/// frames (e.g. `event: endpoint`) that arrive when a server restarts an SSE
/// stream. The default implementation is a no-op, keeping existing behaviour
/// intact.
pub(crate) trait SseStreamReconnect {
    type Error: std::error::Error;
    type Future: Future<Output = Result<BoxedSseResponse, Self::Error>> + Send;
    fn retry_connection(&mut self, last_event_id: Option<&str>) -> Self::Future;
    fn handle_control_event(&mut self, _event: &Sse) -> Result<(), Self::Error> {
        Ok(())
    }
    fn handle_stream_error(
        &mut self,
        error: &(dyn std::error::Error + 'static),
        last_event_id: Option<&str>,
    ) {
        if let Some(id) = last_event_id {
            tracing::warn!(%id, "sse stream error: {error}");
        } else {
            tracing::warn!("sse stream error: {error}");
        }
    }
    fn map_fatal_stream_error(&mut self, error: SseError) -> Option<Self::Error> {
        tracing::warn!("fatal sse stream error: {error}");
        None
    }
}

pin_project_lite::pin_project! {
    pub(crate) struct SseAutoReconnectStream<R>
    where R: SseStreamReconnect
     {
        retry_policy: Arc<dyn SseRetryPolicy>,
        reconnect_only_after_event_id: bool,
        last_event_id: Option<String>,
        server_retry_interval: Option<Duration>,
        connector: R,
        #[pin]
        state: SseAutoReconnectStreamState<R::Future>,
    }
}

impl<R: SseStreamReconnect> SseAutoReconnectStream<R> {
    pub fn new(
        stream: BoxedSseResponse,
        connector: R,
        retry_policy: Arc<dyn SseRetryPolicy>,
    ) -> Self {
        Self {
            retry_policy,
            reconnect_only_after_event_id: false,
            last_event_id: None,
            server_retry_interval: None,
            connector,
            state: SseAutoReconnectStreamState::Connected { stream },
        }
    }

    pub fn new_after_event_id(
        stream: BoxedSseResponse,
        connector: R,
        retry_policy: Arc<dyn SseRetryPolicy>,
    ) -> Self {
        Self {
            retry_policy,
            reconnect_only_after_event_id: true,
            last_event_id: None,
            server_retry_interval: None,
            connector,
            state: SseAutoReconnectStreamState::Connected { stream },
        }
    }
}

impl<E: std::error::Error + Send> SseAutoReconnectStream<NeverReconnect<E>> {
    #[allow(dead_code)]
    pub(crate) fn never_reconnect(stream: BoxedSseResponse, error_when_reconnect: E) -> Self {
        Self {
            retry_policy: Arc::new(NeverRetry),
            reconnect_only_after_event_id: false,
            last_event_id: None,
            server_retry_interval: None,
            connector: NeverReconnect {
                error: Some(error_when_reconnect),
            },
            state: SseAutoReconnectStreamState::Connected { stream },
        }
    }
}

pin_project_lite::pin_project! {
    #[project = SseAutoReconnectStreamStateProj]
    #[non_exhaustive]
    pub enum SseAutoReconnectStreamState<F> {
        Connected {
            #[pin]
            stream: BoxedSseResponse,
        },
        Retrying {
            retry_times: usize,
            #[pin]
            retrying: F,
        },
        WaitingNextRetry {
            #[pin]
            sleep: tokio::time::Sleep,
            retry_times: usize,
        },
        Terminated,
    }
}

impl<R> Stream for SseAutoReconnectStream<R>
where
    R: SseStreamReconnect,
{
    type Item = Result<ServerJsonRpcMessage, R::Error>;
    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let mut this = self.as_mut().project();
        // let this_state = this.state.as_mut().project()
        let state = this.state.as_mut().project();
        let next_state = match state {
            SseAutoReconnectStreamStateProj::Connected { stream } => {
                match ready!(stream.poll_next(cx)) {
                    Some(Ok(sse)) => {
                        if let Some(new_server_retry) = sse.retry {
                            *this.server_retry_interval =
                                Some(Duration::from_millis(new_server_retry));
                        }
                        if let Some(ref event_id) = sse.id {
                            *this.last_event_id = Some(event_id.clone());
                        }
                        // Only treat blank/`message` events as JSON-RPC payloads.
                        // Other control frames (endpoint, ping, etc.) are passed to
                        // the reconnection handler.
                        let is_message_event =
                            matches!(sse.event.as_deref(), None | Some("") | Some("message"));
                        if !is_message_event {
                            match this.connector.handle_control_event(&sse) {
                                Ok(()) => return self.poll_next(cx),
                                Err(e) => {
                                    this.state.set(SseAutoReconnectStreamState::Terminated);
                                    return Poll::Ready(Some(Err(e)));
                                }
                            }
                        }
                        if let Some(data) = sse.data {
                            match serde_json::from_str::<ServerJsonRpcMessage>(&data) {
                                Err(e) => {
                                    // Downgrade to debug to avoid noisy logs when servers emit
                                    // non-JSON payloads as message frames. Include last_event_id
                                    // to aid troubleshooting while keeping default behaviour.
                                    let last_id = this.last_event_id.as_deref().unwrap_or("");
                                    tracing::debug!(last_event_id=%last_id, "failed to deserialize server message: {e}");
                                    return self.poll_next(cx);
                                }
                                Ok(message) => {
                                    return Poll::Ready(Some(Ok(message)));
                                }
                            };
                        } else {
                            return self.poll_next(cx);
                        }
                    }
                    Some(Err(e)) => {
                        if is_event_too_large_error(&e) {
                            this.state.set(SseAutoReconnectStreamState::Terminated);
                            return Poll::Ready(this.connector.map_fatal_stream_error(e).map(Err));
                        }
                        if *this.reconnect_only_after_event_id && this.last_event_id.is_none() {
                            this.state.set(SseAutoReconnectStreamState::Terminated);
                            return Poll::Ready(this.connector.map_fatal_stream_error(e).map(Err));
                        }
                        this.connector
                            .handle_stream_error(&e, this.last_event_id.as_deref());
                        let retrying = this
                            .connector
                            .retry_connection(this.last_event_id.as_deref());
                        SseAutoReconnectStreamState::Retrying {
                            retry_times: 0,
                            retrying,
                        }
                    }
                    None => {
                        if *this.reconnect_only_after_event_id && this.last_event_id.is_none() {
                            tracing::debug!(
                                "sse response ended before an event ID was received; cannot resume"
                            );
                            this.state.set(SseAutoReconnectStreamState::Terminated);
                            return Poll::Ready(None);
                        }
                        // Per SEP-1699, a graceful stream close is
                        // reconnectable.  If the server sent a `retry` field
                        // we MUST wait that long before reconnecting.
                        let interval = this
                            .server_retry_interval
                            .take()
                            .or_else(|| this.retry_policy.retry(0));
                        if let Some(interval) = interval {
                            tracing::debug!(?interval, "sse stream ended gracefully, reconnecting");
                            SseAutoReconnectStreamState::WaitingNextRetry {
                                sleep: tokio::time::sleep(interval),
                                retry_times: 0,
                            }
                        } else {
                            tracing::debug!("sse stream terminated, no reconnect policy");
                            return Poll::Ready(None);
                        }
                    }
                }
            }
            SseAutoReconnectStreamStateProj::Retrying {
                retry_times,
                retrying,
            } => {
                let retry_result = ready!(retrying.poll(cx));
                match retry_result {
                    Ok(new_stream) => SseAutoReconnectStreamState::Connected { stream: new_stream },
                    Err(e) => {
                        tracing::debug!("retry sse stream error: {e}");
                        *retry_times += 1;
                        if let Some(interval) = this.retry_policy.retry(*retry_times) {
                            let interval = this
                                .server_retry_interval
                                .map(|server_retry_interval| server_retry_interval.max(interval))
                                .unwrap_or(interval);
                            let sleep = tokio::time::sleep(interval);
                            SseAutoReconnectStreamState::WaitingNextRetry {
                                sleep,
                                retry_times: *retry_times,
                            }
                        } else {
                            tracing::error!("sse stream error: {e}, max retry times reached");
                            this.state.set(SseAutoReconnectStreamState::Terminated);
                            return Poll::Ready(Some(Err(e)));
                        }
                    }
                }
            }
            SseAutoReconnectStreamStateProj::WaitingNextRetry { sleep, retry_times } => {
                ready!(sleep.poll(cx));
                let retrying = this
                    .connector
                    .retry_connection(this.last_event_id.as_deref());
                let retry_times = *retry_times;
                SseAutoReconnectStreamState::Retrying {
                    retry_times,
                    retrying,
                }
            }
            SseAutoReconnectStreamStateProj::Terminated => {
                return Poll::Ready(None);
            }
        };
        // update the state
        this.state.set(next_state);
        self.poll_next(cx)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[derive(Debug, Error)]
    enum TestReconnectError {
        #[error("SSE stream error: {0}")]
        Sse(SseError),
        #[error("unexpected reconnect")]
        Reconnect,
    }

    struct CountingReconnect {
        attempts: Arc<AtomicUsize>,
    }

    impl SseStreamReconnect for CountingReconnect {
        type Error = TestReconnectError;
        type Future = futures::future::Ready<Result<BoxedSseResponse, Self::Error>>;

        fn retry_connection(&mut self, _last_event_id: Option<&str>) -> Self::Future {
            self.attempts.fetch_add(1, Ordering::Relaxed);
            futures::future::ready(Err(TestReconnectError::Reconnect))
        }

        fn map_fatal_stream_error(&mut self, error: SseError) -> Option<Self::Error> {
            Some(TestReconnectError::Sse(error))
        }
    }

    #[tokio::test]
    async fn bounded_sse_stream_rejects_unterminated_event_over_limit() {
        let source = futures::stream::iter([
            Ok::<_, std::io::Error>(Bytes::from_static(b"data: aaaaa")),
            Ok(Bytes::from_static(b"aaaaaa")),
        ]);
        let mut stream = bounded_sse_stream(source, 16);

        let error = stream.next().await.unwrap().unwrap_err();

        assert!(
            error.to_string().contains("maximum size of 16 bytes"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn bounded_sse_stream_resets_limit_after_event_terminator() {
        let source = futures::stream::iter([
            Ok::<_, std::io::Error>(Bytes::from_static(b"data: a\r")),
            Ok(Bytes::from_static(b"\n\r\ndata: b\n\n")),
        ]);
        let mut stream = bounded_sse_stream(source, 8);

        let first = stream.next().await.unwrap().unwrap();
        let second = stream.next().await.unwrap().unwrap();

        assert_eq!(
            (first.data.as_deref(), second.data.as_deref()),
            (Some("a"), Some("b"))
        );
    }

    #[tokio::test]
    async fn bounded_sse_stream_passes_event_at_exact_limit() {
        let source =
            futures::stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(b"data: ab\n\n"))]);
        let mut stream = bounded_sse_stream(source, 9);

        let event = stream.next().await.unwrap().unwrap();
        assert_eq!(event.data.as_deref(), Some("ab"));
    }

    #[tokio::test]
    async fn bounded_sse_stream_rejects_oversize_split_across_many_chunks() {
        let source = futures::stream::iter(
            std::iter::repeat_with(|| Ok::<_, std::io::Error>(Bytes::from_static(b"data: x")))
                .take(20),
        );
        let mut stream = bounded_sse_stream(source, 32);

        let mut found_error = false;
        while let Some(item) = stream.next().await {
            if item.is_err() {
                found_error = true;
                break;
            }
        }
        assert!(found_error, "expected oversize error");
    }

    #[tokio::test]
    async fn bounded_sse_stream_handles_crlf_split_across_chunks() {
        let source = futures::stream::iter([
            Ok::<_, std::io::Error>(Bytes::from_static(b"data: hello\r")),
            Ok(Bytes::from_static(b"\n\ndata: world\n\n")),
        ]);
        let mut stream = bounded_sse_stream(source, 64);

        let first = stream.next().await.unwrap().unwrap();
        let second = stream.next().await.unwrap().unwrap();

        assert_eq!(first.data.as_deref(), Some("hello"));
        assert_eq!(second.data.as_deref(), Some("world"));
    }

    #[tokio::test]
    async fn bounded_sse_stream_discards_completed_comment_lines_from_limit() {
        let source = futures::stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(
            b": ping\n: ping\n: ping\n",
        ))]);
        let mut stream = bounded_sse_stream(source, 6);

        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn bounded_sse_stream_comments_do_not_reset_accumulated_data() {
        let source = futures::stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(
            b"data: a\n: ping\ndata: b\n",
        ))]);
        let mut stream = bounded_sse_stream(source, 14);

        assert!(stream.next().await.unwrap().is_err());
    }

    #[tokio::test]
    async fn bounded_sse_stream_counts_multiline_data_join_newlines() {
        let source = futures::stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(
            b"data: aaa\ndata: bbb\n\n",
        ))]);
        let mut stream = bounded_sse_stream(source, 18);

        let error = stream.next().await.unwrap().unwrap_err();
        assert!(
            error.to_string().contains("maximum size"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn bounded_sse_stream_propagates_source_error() {
        let source = futures::stream::iter([Err::<Bytes, _>(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "connection reset",
        ))]);
        let mut stream = bounded_sse_stream(source, 1024);

        let error = stream.next().await.unwrap().unwrap_err();
        assert!(
            error.to_string().contains("connection reset"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn is_event_too_large_error_detects_oversize() {
        let source = futures::stream::iter([Ok::<_, std::io::Error>(Bytes::from(vec![b'A'; 100]))]);
        let mut stream = bounded_sse_stream(source, 8);

        let error = stream.next().await.unwrap().unwrap_err();
        assert!(is_event_too_large_error(&error));
    }

    #[tokio::test]
    async fn is_event_too_large_error_rejects_other_errors() {
        let source =
            futures::stream::iter([Err::<Bytes, _>(std::io::Error::other("something else"))]);
        let mut stream = bounded_sse_stream(source, 1024);

        let error = stream.next().await.unwrap().unwrap_err();
        assert!(!is_event_too_large_error(&error));
    }

    #[tokio::test]
    async fn oversized_event_returns_error_without_reconnecting() {
        let source = futures::stream::iter([Ok::<_, std::io::Error>(Bytes::from(vec![b'A'; 100]))]);
        let attempts = Arc::new(AtomicUsize::new(0));
        let connector = CountingReconnect {
            attempts: attempts.clone(),
        };
        let stream = SseAutoReconnectStream::new(
            bounded_sse_stream(source, 8),
            connector,
            Arc::new(NeverRetry),
        );
        let mut stream = std::pin::pin!(stream);

        let result = stream.next().await;

        assert!(
            matches!(result, Some(Err(TestReconnectError::Sse(_))))
                && attempts.load(Ordering::Relaxed) == 0
        );
    }

    #[tokio::test]
    async fn response_without_event_id_does_not_reconnect() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let connector = CountingReconnect {
            attempts: attempts.clone(),
        };
        let stream = SseAutoReconnectStream::new_after_event_id(
            futures::stream::empty().boxed(),
            connector,
            Arc::new(FixedInterval {
                max_times: Some(1),
                duration: Duration::ZERO,
            }),
        );
        let mut stream = std::pin::pin!(stream);

        assert!(stream.next().await.is_none());
        assert_eq!(attempts.load(Ordering::Relaxed), 0);
    }
}
