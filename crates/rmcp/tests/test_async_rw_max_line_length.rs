//! Regression tests for the line-length bound on `AsyncRwTransport`.
//!
//! The stdio server transport and the `TokioChildProcess` client transport both
//! read through `AsyncRwTransport`. Before this bound existed, the read side
//! buffered an incoming line with no ceiling, so a peer that sent an
//! unterminated or oversized line could grow the process's memory until it was
//! killed. See https://github.com/modelcontextprotocol/rust-sdk/issues/1030.

use rmcp::{
    RoleServer,
    transport::{
        Transport,
        async_rw::{AsyncRwTransport, DEFAULT_MAX_LINE_LENGTH},
    },
};
use tokio::io::{AsyncWriteExt, DuplexStream};

const MAX: usize = 4 * 1024;

/// A single-line JSON-RPC request padded out to at least `total` bytes.
fn padded_request(id: u64, total: usize) -> String {
    let base = format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"ping","params":{{"pad":""}}}}"#);
    let pad = total.saturating_sub(base.len());
    format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"ping","params":{{"pad":"{}"}}}}"#,
        "x".repeat(pad)
    )
}

fn small_request(id: u64) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"ping"}}"#)
}

fn transport(max_line_length: usize) -> (DuplexStream, impl Transport<RoleServer>) {
    let (peer, ours) = tokio::io::duplex(64 * 1024);
    let transport = AsyncRwTransport::<RoleServer, _, _>::new(ours, tokio::io::sink())
        .with_max_line_length(max_line_length);
    (peer, transport)
}

fn received_id(msg: &rmcp::service::RxJsonRpcMessage<RoleServer>) -> serde_json::Value {
    serde_json::to_value(msg).expect("received message is serializable")["id"].clone()
}

/// A *valid* message over the limit must be dropped rather than delivered, and
/// the stream must keep working afterwards.
///
/// Using a well-formed message matters: unparsable junk is discarded by the
/// existing error handling either way, so only a valid oversized message
/// distinguishes a bounded read from an unbounded one.
#[tokio::test]
async fn oversized_message_is_dropped_and_stream_recovers() {
    let (mut peer, mut transport) = transport(MAX);

    tokio::spawn(async move {
        peer.write_all(padded_request(1, MAX * 4).as_bytes())
            .await
            .unwrap();
        peer.write_all(b"\n").await.unwrap();
        peer.write_all(small_request(2).as_bytes()).await.unwrap();
        peer.write_all(b"\n").await.unwrap();
    });

    let msg = transport.receive().await.expect("second message delivered");
    assert_eq!(
        received_id(&msg),
        serde_json::json!(2),
        "the oversized message should have been dropped, not delivered"
    );
}

/// The DoS shape from the report: bytes keep arriving with no newline at all.
/// The transport must not accumulate them, and must recover once a delimiter
/// finally shows up.
#[tokio::test]
async fn unterminated_flood_is_discarded_and_stream_recovers() {
    let (mut peer, mut transport) = transport(MAX);

    tokio::spawn(async move {
        let chunk = vec![b'A'; 8 * 1024];
        // Well past the limit, still no newline.
        for _ in 0..16 {
            peer.write_all(&chunk).await.unwrap();
        }
        peer.write_all(b"\n").await.unwrap();
        peer.write_all(small_request(7).as_bytes()).await.unwrap();
        peer.write_all(b"\n").await.unwrap();
    });

    let msg = transport
        .receive()
        .await
        .expect("message after the flood is delivered");
    assert_eq!(received_id(&msg), serde_json::json!(7));
}

/// A message that fits must still be delivered, including right at the boundary.
#[tokio::test]
async fn message_within_the_limit_is_delivered() {
    let (mut peer, mut transport) = transport(MAX);

    // `MAX` counts the trailing newline too, so this is the largest line that fits.
    let line = padded_request(3, MAX - 1);
    assert_eq!(line.len(), MAX - 1);

    tokio::spawn(async move {
        peer.write_all(line.as_bytes()).await.unwrap();
        peer.write_all(b"\n").await.unwrap();
    });

    let msg = transport.receive().await.expect("message delivered");
    assert_eq!(received_id(&msg), serde_json::json!(3));
}

/// The default must be generous enough for real payloads, e.g. an embedded
/// image, so existing callers are not broken by the bound being introduced.
#[tokio::test]
async fn default_limit_accepts_a_large_realistic_message() {
    let (peer, ours) = tokio::io::duplex(64 * 1024);
    let mut transport = AsyncRwTransport::<RoleServer, _, _>::new(ours, tokio::io::sink());
    let mut peer = peer;

    assert_eq!(DEFAULT_MAX_LINE_LENGTH, 16 * 1024 * 1024);

    tokio::spawn(async move {
        peer.write_all(padded_request(4, 1024 * 1024).as_bytes())
            .await
            .unwrap();
        peer.write_all(b"\n").await.unwrap();
    });

    let msg = transport.receive().await.expect("1 MiB message delivered");
    assert_eq!(received_id(&msg), serde_json::json!(4));
}

/// The bounded read replaced `read_until`, which used to be what carried a
/// partially read line across cancellations. A line arriving in several chunks
/// must still be reassembled.
#[tokio::test]
async fn line_split_across_many_reads_is_reassembled() {
    let (mut peer, mut transport) = transport(MAX);

    let line = padded_request(5, MAX / 2);

    tokio::spawn(async move {
        for chunk in line.as_bytes().chunks(97) {
            peer.write_all(chunk).await.unwrap();
            tokio::task::yield_now().await;
        }
        peer.write_all(b"\n").await.unwrap();
    });

    let msg = transport.receive().await.expect("reassembled message");
    assert_eq!(received_id(&msg), serde_json::json!(5));
}

/// Two oversized messages in a row must not wedge the transport.
#[tokio::test]
async fn consecutive_oversized_messages_still_recover() {
    let (mut peer, mut transport) = transport(MAX);

    tokio::spawn(async move {
        for id in [1, 2] {
            peer.write_all(padded_request(id, MAX * 3).as_bytes())
                .await
                .unwrap();
            peer.write_all(b"\n").await.unwrap();
        }
        peer.write_all(small_request(9).as_bytes()).await.unwrap();
        peer.write_all(b"\n").await.unwrap();
    });

    let msg = transport
        .receive()
        .await
        .expect("message after two oversized");
    assert_eq!(received_id(&msg), serde_json::json!(9));
}

/// Negative control for the bound itself.
///
/// `usize::MAX` is the pre-fix behaviour, and with it the very same oversized
/// message is delivered instead of dropped. This is what makes
/// `oversized_message_is_dropped_and_stream_recovers` meaningful: the outcome
/// changes only because of the limit.
#[tokio::test]
async fn unbounded_limit_still_delivers_an_oversized_message() {
    let (mut peer, mut transport) = transport(usize::MAX);

    tokio::spawn(async move {
        peer.write_all(padded_request(1, MAX * 4).as_bytes())
            .await
            .unwrap();
        peer.write_all(b"\n").await.unwrap();
        peer.write_all(small_request(2).as_bytes()).await.unwrap();
        peer.write_all(b"\n").await.unwrap();
    });

    let msg = transport.receive().await.expect("message delivered");
    assert_eq!(
        received_id(&msg),
        serde_json::json!(1),
        "with no bound the oversized message is buffered and delivered, which is the behaviour the bound removes"
    );
}
