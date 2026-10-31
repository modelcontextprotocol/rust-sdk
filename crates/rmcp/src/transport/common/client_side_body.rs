//! Bounded buffering for the built-in Streamable HTTP clients.

use bytes::Bytes;
use futures::{Stream, StreamExt};

use crate::transport::streamable_http_client::StreamableHttpError;

/// Buffer a response without appending a chunk that would exceed `limit`.
///
/// A known content length permits early rejection, but is never trusted as the
/// sole bound. The count applies to bytes yielded by the HTTP backend (after
/// decompression, when enabled). The backend may already have allocated a chunk;
/// this helper bounds the accumulated body, not every backend allocation.
pub(crate) async fn read_bounded_body<S, E>(
    stream: S,
    content_length: Option<u64>,
    limit: usize,
) -> Result<Vec<u8>, StreamableHttpError<E>>
where
    S: Stream<Item = Result<Bytes, E>>,
    E: std::error::Error + Send + Sync + 'static,
{
    if content_length.is_some_and(|length| length > limit as u64) {
        return Err(StreamableHttpError::ResponseBodyTooLarge { limit });
    }
    let mut body = Vec::new();
    futures::pin_mut!(stream);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(StreamableHttpError::Client)?;
        if chunk.len() > limit - body.len() {
            return Err(StreamableHttpError::ResponseBodyTooLarge { limit });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Read at most `limit` bytes for a diagnostic, dropping the rest of the stream.
///
/// Unlike a protocol message, a diagnostic need not be complete. Stop as soon as
/// the prefix is full, without polling for another chunk or EOF. Content-Length
/// is not needed: an oversized declaration must not discard a useful prefix.
/// The returned bytes must never be parsed as a complete JSON-RPC message.
pub(crate) async fn read_truncated_body<S, E>(
    stream: S,
    limit: usize,
) -> Result<Vec<u8>, StreamableHttpError<E>>
where
    S: Stream<Item = Result<Bytes, E>>,
    E: std::error::Error + Send + Sync + 'static,
{
    let mut body = Vec::new();
    futures::pin_mut!(stream);
    while body.len() < limit {
        let Some(chunk) = stream.next().await else {
            break;
        };
        let chunk = chunk.map_err(StreamableHttpError::Client)?;
        let count = chunk.len().min(limit - body.len());
        body.extend_from_slice(&chunk[..count]);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use std::{
        io,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use super::*;

    #[tokio::test]
    async fn exact_limit_accepts_multiple_chunks() {
        let stream = futures::stream::iter([
            Ok::<_, io::Error>(Bytes::from_static(b"ab")),
            Ok(Bytes::from_static(b"cd")),
        ]);
        assert_eq!(read_bounded_body(stream, None, 4).await.unwrap(), b"abcd");
    }

    #[tokio::test]
    async fn declared_oversize_is_rejected_without_polling() {
        let stream =
            futures::stream::poll_fn(|_| -> std::task::Poll<Option<Result<Bytes, io::Error>>> {
                panic!("a declared oversized body must not be polled")
            });
        assert!(matches!(
            read_bounded_body(stream, Some(5), 4).await,
            Err(StreamableHttpError::ResponseBodyTooLarge { limit: 4 })
        ));
    }

    #[tokio::test]
    async fn unknown_length_stops_at_first_oversized_chunk() {
        let polls = Arc::new(AtomicUsize::new(0));
        let count = polls.clone();
        let stream = futures::stream::poll_fn(move |_| {
            let index = count.fetch_add(1, Ordering::SeqCst);
            assert!(index < 2, "must not drain an oversized or endless body");
            std::task::Poll::Ready(Some(Ok::<_, io::Error>(Bytes::from_static(b"abc"))))
        });
        assert!(matches!(
            read_bounded_body(stream, None, 4).await,
            Err(StreamableHttpError::ResponseBodyTooLarge { limit: 4 })
        ));
        assert_eq!(polls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn understated_length_does_not_bypass_chunk_counting() {
        let stream = futures::stream::iter([Ok::<_, io::Error>(Bytes::from_static(b"abcde"))]);
        assert!(matches!(
            read_bounded_body(stream, Some(1), 4).await,
            Err(StreamableHttpError::ResponseBodyTooLarge { limit: 4 })
        ));
    }

    #[tokio::test]
    async fn zero_limit_accepts_only_empty_bodies() {
        assert!(
            read_bounded_body(
                futures::stream::empty::<Result<Bytes, io::Error>>(),
                None,
                0
            )
            .await
            .unwrap()
            .is_empty()
        );
        let stream = futures::stream::iter([Ok::<_, io::Error>(Bytes::from_static(b"x"))]);
        assert!(matches!(
            read_bounded_body(stream, None, 0).await,
            Err(StreamableHttpError::ResponseBodyTooLarge { limit: 0 })
        ));
    }

    #[tokio::test]
    async fn stream_failure_remains_a_client_error() {
        let stream = futures::stream::iter([Err::<Bytes, _>(io::Error::other("read failed"))]);
        assert!(matches!(
            read_bounded_body(stream, None, 8).await,
            Err(StreamableHttpError::Client(_))
        ));
    }

    #[tokio::test]
    async fn truncated_body_keeps_short_bodies_complete() {
        let stream = futures::stream::iter([
            Ok::<_, io::Error>(Bytes::from_static(b"ab")),
            Ok(Bytes::from_static(b"c")),
        ]);
        assert_eq!(read_truncated_body(stream, 4).await.unwrap(), b"abc");
    }

    #[tokio::test]
    async fn truncated_body_stops_at_exact_limit_without_polling_for_eof() {
        let stream = futures::stream::iter([
            Ok::<_, io::Error>(Bytes::from_static(b"ab")),
            Ok(Bytes::from_static(b"cd")),
        ])
        .chain(futures::stream::poll_fn(|_| {
            panic!("a full diagnostic prefix must not poll for EOF")
        }));
        assert_eq!(read_truncated_body(stream, 4).await.unwrap(), b"abcd");
    }

    #[tokio::test]
    async fn truncated_body_stops_in_the_middle_of_an_oversized_chunk() {
        let polls = Arc::new(AtomicUsize::new(0));
        let count = polls.clone();
        let stream = futures::stream::poll_fn(move |_| {
            assert!(
                count.fetch_add(1, Ordering::SeqCst) < 2,
                "must not drain the body"
            );
            std::task::Poll::Ready(Some(Ok::<_, io::Error>(Bytes::from_static(b"abc"))))
        });
        assert_eq!(read_truncated_body(stream, 4).await.unwrap(), b"abca");
        assert_eq!(polls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn truncated_body_zero_limit_does_not_poll() {
        let stream =
            futures::stream::poll_fn(|_| -> std::task::Poll<Option<Result<Bytes, io::Error>>> {
                panic!("zero diagnostic limit must not read the body")
            });
        assert!(read_truncated_body(stream, 0).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn truncated_body_preserves_io_errors_before_the_limit() {
        let stream = futures::stream::iter([
            Ok(Bytes::from_static(b"a")),
            Err(io::Error::other("read failed")),
        ]);
        assert!(matches!(
            read_truncated_body(stream, 4).await,
            Err(StreamableHttpError::Client(_))
        ));
    }

    #[tokio::test]
    async fn truncated_body_counts_bytes_not_utf8_characters() {
        let stream =
            futures::stream::iter([Ok::<_, io::Error>(Bytes::from_static("é".as_bytes()))]);
        assert_eq!(read_truncated_body(stream, 1).await.unwrap(), [0xc3]);
    }
}
