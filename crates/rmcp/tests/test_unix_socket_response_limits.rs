#![cfg(all(
    unix,
    feature = "transport-streamable-http-client-unix-socket",
    not(feature = "local")
))]

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use rmcp::{
    model::{ClientJsonRpcMessage, ClientRequest, PingRequest, RequestId},
    transport::{
        UnixSocketHttpClient,
        streamable_http_client::{StreamableHttpClient, StreamableHttpError},
    },
};
use rstest::rstest;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixListener,
    task::JoinHandle,
};

struct Server {
    path: PathBuf,
    task: JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_file(&self.path);
    }
}

async fn oversized_response(status: u16, limit: usize, known_length: bool) -> Server {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "rmcp-body-limit-{}-{}.sock",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let listener = UnixListener::bind(&path).unwrap();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            let size = socket.read(&mut buffer).await.unwrap();
            assert!(size > 0 && request.len() + size <= 8192);
            request.extend_from_slice(&buffer[..size]);
        }
        let framing = if known_length {
            format!("Content-Length: {}\r\n", limit + 1)
        } else {
            "Transfer-Encoding: chunked\r\n".to_owned()
        };
        let headers =
            format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\n{framing}\r\n");
        socket.write_all(headers.as_bytes()).await.unwrap();
        if !known_length {
            let chunk = vec![b' '; 4096];
            for _ in 0..limit / chunk.len() {
                socket.write_all(b"1000\r\n").await.unwrap();
                socket.write_all(&chunk).await.unwrap();
                socket.write_all(b"\r\n").await.unwrap();
            }
            socket.write_all(b"1\r\n \r\n").await.unwrap();
        }
        // No body at all for a declared oversize; no terminating chunk otherwise.
        // The client must reject without waiting for EOF in either case.
        std::future::pending::<()>().await;
        drop(socket);
    });
    Server { path, task }
}

#[rstest]
#[case::json_declared(200, 16 * 1024 * 1024, true)]
#[case::json_streamed(200, 16 * 1024 * 1024, false)]
#[case::error_declared(400, 64 * 1024, true)]
#[case::error_streamed(400, 64 * 1024, false)]
#[case::server_error_streamed(500, 64 * 1024, false)]
#[tokio::test]
async fn unix_socket_rejects_oversized_response_before_eof(
    #[case] status: u16,
    #[case] limit: usize,
    #[case] known_length: bool,
) {
    let server = oversized_response(status, limit, known_length).await;
    let uri: Arc<str> = Arc::from("http://localhost/mcp");
    let client = UnixSocketHttpClient::new(server.path.to_str().unwrap(), &uri);
    let message = ClientJsonRpcMessage::request(
        ClientRequest::PingRequest(PingRequest::default()),
        RequestId::Number(1),
    );
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        client.post_message(uri, message, None, None, Default::default()),
    )
    .await
    .expect("oversized response must be rejected before EOF");
    assert!(
        matches!(result, Err(StreamableHttpError::ResponseBodyTooLarge { max_size }) if max_size == limit),
        "expected response size error, got {result:?}"
    );
}
