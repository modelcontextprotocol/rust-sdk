//! Bounded collection of non-streaming HTTP responses.

use bytes::Bytes;
use futures::{Stream, TryStreamExt};

use crate::transport::streamable_http_client::StreamableHttpError;

/// Compare the media-type essence of a `Content-Type` value against `expected`.
///
/// Media types are case-insensitive (RFC 9110) and parameters such as
/// `; charset=utf-8` do not change the type, so a byte-prefix check rejects
/// valid responses such as `Application/JSON`.
pub(super) fn media_type_is(value: &[u8], expected: &str) -> bool {
    value
        .split(|byte| *byte == b';')
        .next()
        .and_then(|essence| std::str::from_utf8(essence).ok())
        .is_some_and(|essence| {
            essence
                .trim_matches([' ', '\t'])
                .eq_ignore_ascii_case(expected)
        })
}

/// Count bytes before retaining them; a length hint can reject a body early,
/// but never replaces the streamed check or determines allocation size.
pub(super) async fn read_bounded_body<S, E>(
    stream: S,
    content_length: Option<u64>,
    max_size: usize,
) -> Result<Vec<u8>, StreamableHttpError<E>>
where
    S: Stream<Item = Result<Bytes, E>>,
    E: std::error::Error + Send + Sync + 'static,
{
    if content_length.is_some_and(|length| length > max_size as u64) {
        return Err(StreamableHttpError::ResponseBodyTooLarge { max_size });
    }

    futures::pin_mut!(stream);
    let mut body = Vec::new();
    while let Some(chunk) = stream
        .try_next()
        .await
        .map_err(StreamableHttpError::Client)?
    {
        if chunk.len() > max_size - body.len() {
            return Err(StreamableHttpError::ResponseBodyTooLarge { max_size });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Read at most `max_size` bytes for a diagnostic, dropping the rest of the stream.
///
/// Unlike a protocol message, a diagnostic need not be complete. Stop as soon as
/// the prefix is full, without polling for another chunk or EOF. `Content-Length`
/// is not consulted: an oversized declaration must not discard a useful prefix.
/// The returned bytes must never be parsed as a complete JSON-RPC message.
pub(super) async fn read_truncated_body<S, E>(
    stream: S,
    max_size: usize,
) -> Result<Vec<u8>, StreamableHttpError<E>>
where
    S: Stream<Item = Result<Bytes, E>>,
    E: std::error::Error + Send + Sync + 'static,
{
    let mut body = Vec::new();
    futures::pin_mut!(stream);
    while body.len() < max_size {
        let Some(chunk) = stream
            .try_next()
            .await
            .map_err(StreamableHttpError::Client)?
        else {
            break;
        };
        let count = chunk.len().min(max_size - body.len());
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

    use futures::stream;
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case(None)]
    #[case(Some(8))]
    #[tokio::test]
    async fn accepts_exact_limit_across_chunks(#[case] length: Option<u64>) {
        let chunks = ["abcd", "", "efgh"].map(|chunk| Ok::<_, io::Error>(Bytes::from(chunk)));
        let body = read_bounded_body(stream::iter(chunks), length, 8)
            .await
            .unwrap();
        assert_eq!(body, b"abcdefgh");
    }

    #[rstest]
    #[case(None)]
    #[case(Some(1))]
    #[tokio::test]
    async fn counts_bytes_even_with_a_small_length_hint(#[case] length: Option<u64>) {
        let chunks = ["abcd", "efgh"].map(|chunk| Ok::<_, io::Error>(Bytes::from(chunk)));
        let error = read_bounded_body(stream::iter(chunks), length, 7)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            StreamableHttpError::ResponseBodyTooLarge { max_size: 7 }
        ));
    }

    #[tokio::test]
    async fn oversized_length_does_not_poll_the_body() {
        let stream = stream::poll_fn::<Result<Bytes, io::Error>, _>(|_| {
            panic!("an oversized Content-Length must be rejected before polling")
        });
        let error = read_bounded_body(stream, Some(u64::MAX), 8)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            StreamableHttpError::ResponseBodyTooLarge { max_size: 8 }
        ));
    }

    #[tokio::test]
    async fn overflowing_chunk_does_not_poll_again() {
        let first = stream::once(async { Ok::<_, io::Error>(Bytes::from_static(b"too large")) });
        let rest = stream::poll_fn::<Result<Bytes, io::Error>, _>(|_| {
            panic!("must stop reading as soon as the limit is crossed")
        });
        let error = read_bounded_body(futures::StreamExt::chain(first, rest), None, 8)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            StreamableHttpError::ResponseBodyTooLarge { max_size: 8 }
        ));
    }

    #[tokio::test]
    async fn preserves_body_read_errors() {
        let stream = stream::iter([Err::<Bytes, _>(io::Error::other("body read failed"))]);
        let error = read_bounded_body(stream, None, 8).await.unwrap_err();
        assert!(matches!(error, StreamableHttpError::Client(_)));
    }

    #[tokio::test]
    async fn accepts_empty_body_with_zero_limit() {
        let stream = stream::empty::<Result<Bytes, io::Error>>();
        assert!(
            read_bounded_body(stream, Some(0), 0)
                .await
                .unwrap()
                .is_empty()
        );
        let stream = stream::iter([Ok::<_, io::Error>(Bytes::from_static(b"x"))]);
        assert!(matches!(
            read_bounded_body(stream, None, 0).await,
            Err(StreamableHttpError::ResponseBodyTooLarge { max_size: 0 })
        ));
    }

    #[tokio::test]
    async fn truncated_body_keeps_short_bodies_complete() {
        let stream = stream::iter([
            Ok::<_, io::Error>(Bytes::from_static(b"ab")),
            Ok(Bytes::from_static(b"c")),
        ]);
        assert_eq!(read_truncated_body(stream, 4).await.unwrap(), b"abc");
    }

    #[tokio::test]
    async fn truncated_body_stops_at_exact_limit_without_polling_for_eof() {
        let stream = futures::StreamExt::chain(
            stream::iter([
                Ok::<_, io::Error>(Bytes::from_static(b"ab")),
                Ok(Bytes::from_static(b"cd")),
            ]),
            stream::poll_fn(|_| panic!("a full diagnostic prefix must not poll for EOF")),
        );
        assert_eq!(read_truncated_body(stream, 4).await.unwrap(), b"abcd");
    }

    #[tokio::test]
    async fn truncated_body_stops_in_the_middle_of_an_oversized_chunk() {
        let polls = Arc::new(AtomicUsize::new(0));
        let count = polls.clone();
        let stream = stream::poll_fn(move |_| {
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
        let stream = stream::poll_fn(|_| -> std::task::Poll<Option<Result<Bytes, io::Error>>> {
            panic!("zero diagnostic limit must not read the body")
        });
        assert!(read_truncated_body(stream, 0).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn truncated_body_preserves_io_errors_before_the_limit() {
        let stream = stream::iter([
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
        let stream = stream::iter([Ok::<_, io::Error>(Bytes::from_static("é".as_bytes()))]);
        assert_eq!(read_truncated_body(stream, 1).await.unwrap(), [0xc3]);
    }

    #[rstest]
    #[case(b"application/json")]
    #[case(b"Application/JSON")]
    #[case(b"APPLICATION/JSON; charset=utf-8")]
    #[case(b" application/json ")]
    fn media_type_is_case_insensitive_and_ignores_parameters(#[case] value: &[u8]) {
        assert!(media_type_is(value, "application/json"));
        assert!(!media_type_is(value, "text/event-stream"));
    }

    #[rstest]
    #[case(b"TEXT/EVENT-STREAM;charset=UTF-8", "text/event-stream", true)]
    #[case(b"application/jsonx", "application/json", false)]
    #[case(b"application/json-patch+json", "application/json", false)]
    #[case(b"application/json;", "application/json", true)]
    #[case(b"", "application/json", false)]
    fn media_type_is_matches_the_essence_only(
        #[case] value: &[u8],
        #[case] expected: &str,
        #[case] matches: bool,
    ) {
        assert_eq!(media_type_is(value, expected), matches);
    }
}
