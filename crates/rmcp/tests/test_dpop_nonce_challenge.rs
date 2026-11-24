#![cfg(all(
    feature = "client",
    feature = "transport-streamable-http-client-reqwest"
))]
//! A 401 challenge's `DPoP-Nonce` header reaches `AuthRequiredError`.

use std::{collections::HashMap, sync::Arc};

use axum::{Router, body::Body, http::Response, routing::any};
use rmcp::{
    model::{ClientJsonRpcMessage, ClientRequest, PingRequest, RequestId},
    transport::streamable_http_client::{StreamableHttpClient, StreamableHttpError},
};

const CHALLENGE: &str = r#"DPoP error="use_dpop_nonce", algs="ES256""#;

fn ping() -> ClientJsonRpcMessage {
    ClientJsonRpcMessage::request(
        ClientRequest::PingRequest(PingRequest::default()),
        RequestId::Number(1),
    )
}

/// A server whose `/nonce`, `/blank`, `/invalid` and `/none` routes answer 401
/// with a `DPoP-Nonce` value, an empty one, one with a space, and none.
async fn challenge_server() -> String {
    fn challenge(nonce: Option<&'static str>) -> Response<Body> {
        let mut builder = Response::builder()
            .status(401)
            .header("www-authenticate", CHALLENGE);
        if let Some(nonce) = nonce {
            builder = builder.header("dpop-nonce", nonce);
        }
        builder.body(Body::empty()).unwrap()
    }
    let router = Router::new()
        .route("/nonce", any(|| async { challenge(Some("server-nonce")) }))
        .route("/blank", any(|| async { challenge(Some("")) }))
        .route("/none", any(|| async { challenge(None) }))
        .route(
            "/invalid",
            any(|| async { challenge(Some("server nonce")) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{addr}")
}

async fn post_nonce(base: &str, route: &str) -> Option<String> {
    let result = reqwest::Client::new()
        .post_message(
            Arc::from(format!("{base}{route}").as_str()),
            ping(),
            None,
            None,
            HashMap::new(),
        )
        .await;
    match result {
        Err(StreamableHttpError::AuthRequired(error)) => {
            assert_eq!(error.www_authenticate_header, CHALLENGE);
            error.dpop_nonce
        }
        Ok(_) => panic!("expected an auth challenge"),
        Err(error) => panic!("expected an auth challenge, got {error:?}"),
    }
}

async fn get_nonce(base: &str, route: &str) -> Option<String> {
    let result = reqwest::Client::new()
        .get_stream(
            Arc::from(format!("{base}{route}").as_str()),
            None,
            None,
            None,
            HashMap::new(),
        )
        .await;
    match result {
        Err(StreamableHttpError::AuthRequired(error)) => error.dpop_nonce,
        Ok(_) => panic!("expected an auth challenge"),
        Err(error) => panic!("expected an auth challenge, got {error:?}"),
    }
}

#[tokio::test]
async fn post_401_carries_the_server_nonce() {
    let base = challenge_server().await;
    assert_eq!(
        post_nonce(&base, "/nonce").await.as_deref(),
        Some("server-nonce")
    );
    assert_eq!(post_nonce(&base, "/blank").await, None);
    assert_eq!(post_nonce(&base, "/none").await, None);
    assert_eq!(post_nonce(&base, "/invalid").await, None);
}

#[tokio::test]
async fn get_401_carries_the_server_nonce() {
    let base = challenge_server().await;
    assert_eq!(
        get_nonce(&base, "/nonce").await.as_deref(),
        Some("server-nonce")
    );
    assert_eq!(get_nonce(&base, "/blank").await, None);
    assert_eq!(get_nonce(&base, "/none").await, None);
}

#[cfg(all(unix, feature = "transport-streamable-http-client-unix-socket"))]
mod unix_socket {
    use rmcp::transport::UnixSocketHttpClient;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::UnixListener,
    };

    use super::*;

    const URI: &str = "http://localhost/mcp";

    /// Serves `count` connections, each answered with a 401 challenge that
    /// carries `nonce` as `DPoP-Nonce` when given. The socket lives in `dir`, which
    /// is removed when it drops, including when an assertion panics.
    fn challenge_socket(
        dir: &tempfile::TempDir,
        count: usize,
        nonce: Option<&'static str>,
    ) -> std::path::PathBuf {
        let path = dir.path().join("mcp.sock");
        let listener = UnixListener::bind(&path).unwrap();
        tokio::spawn(async move {
            for _ in 0..count {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                loop {
                    let read = stream.read(&mut buffer).await.unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                        let length = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length:"))
                            .map(|value| value.trim().parse::<usize>().unwrap())
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let mut response = format!(
                    "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: {CHALLENGE}\r\n\
                     Connection: close\r\nContent-Length: 0\r\n"
                );
                if let Some(nonce) = nonce {
                    response.push_str(&format!("DPoP-Nonce: {nonce}\r\n"));
                }
                response.push_str("\r\n");
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        path
    }

    async fn nonces(nonce: Option<&'static str>) -> (Option<String>, Option<String>) {
        let dir = tempfile::tempdir().unwrap();
        let path = challenge_socket(&dir, 2, nonce);
        let client = UnixSocketHttpClient::new(path.to_str().unwrap(), URI);
        let posted = match client
            .post_message(Arc::from(URI), ping(), None, None, HashMap::new())
            .await
        {
            Err(StreamableHttpError::AuthRequired(error)) => error.dpop_nonce,
            Ok(_) => panic!("expected an auth challenge"),
            Err(error) => panic!("expected an auth challenge, got {error:?}"),
        };
        let streamed = match client
            .get_stream(Arc::from(URI), None, None, None, HashMap::new())
            .await
        {
            Err(StreamableHttpError::AuthRequired(error)) => error.dpop_nonce,
            Ok(_) => panic!("expected an auth challenge"),
            Err(error) => panic!("expected an auth challenge, got {error:?}"),
        };
        (posted, streamed)
    }

    #[tokio::test]
    async fn unix_socket_401_carries_the_server_nonce() {
        let (posted, streamed) = nonces(Some("server-nonce")).await;
        assert_eq!(posted.as_deref(), Some("server-nonce"));
        assert_eq!(streamed.as_deref(), Some("server-nonce"));
    }

    #[tokio::test]
    async fn unix_socket_401_without_a_nonce_has_none() {
        assert_eq!(nonces(None).await, (None, None));
        assert_eq!(nonces(Some("")).await, (None, None));
    }
}
