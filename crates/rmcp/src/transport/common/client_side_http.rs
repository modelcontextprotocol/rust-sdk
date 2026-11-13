//! Bounded collection of non-streaming HTTP responses.

use bytes::Bytes;
use futures::{Stream, TryStreamExt};

use crate::transport::streamable_http_client::StreamableHttpError;

pub(super) const MAX_JSON_RESPONSE_SIZE: usize = 16 * 1024 * 1024;
pub(super) const MAX_ERROR_RESPONSE_SIZE: usize = 64 * 1024;

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

#[cfg(test)]
mod tests {
    use std::io;

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
    }
}
