#![cfg(all(
    feature = "auth-dpop",
    feature = "transport-streamable-http-client",
    feature = "transport-streamable-http-client-reqwest"
))]

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Response},
    routing::any,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use oauth2::{AccessToken, basic::BasicTokenType};
use rmcp::{
    model::{ClientJsonRpcMessage, ClientRequest, PingRequest, RequestId},
    transport::{
        auth::{
            AuthClient, AuthorizationManager, CredentialStore, InMemoryCredentialStore,
            OAuthTokenResponse, StoredCredentials, VendorExtraTokenFields, dpop::Es256DpopSigner,
        },
        streamable_http_client::{StreamableHttpClient, StreamableHttpPostResponse},
    },
};

/// The `ath` claim: base64url SHA-256 of the access token (RFC 9449 §4.2).
fn access_token_hash(access_token: &str) -> String {
    use sha2::{Digest, Sha256};
    URL_SAFE_NO_PAD.encode(Sha256::digest(access_token.as_bytes()))
}

const CHALLENGE: &str = r#"DPoP error="use_dpop_nonce", algs="ES256""#;

fn ping() -> ClientJsonRpcMessage {
    ClientJsonRpcMessage::request(
        ClientRequest::PingRequest(PingRequest::default()),
        RequestId::Number(1),
    )
}

fn proof_claims(proof: &str) -> serde_json::Value {
    let claims = proof.split('.').nth(1).expect("proof has claims");
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(claims).unwrap()).unwrap()
}

#[tokio::test]
async fn auth_client_sends_dpop_headers_and_retries_with_the_server_nonce() {
    let seen: Arc<Mutex<Vec<HeaderMap>>> = Arc::default();
    let captured = seen.clone();
    let router = Router::new().route(
        "/mcp",
        any(move |headers: HeaderMap| {
            let captured = captured.clone();
            async move {
                let mut seen = captured.lock().unwrap();
                seen.push(headers);
                if seen.len() == 1 {
                    Response::builder()
                        .status(401)
                        .header("www-authenticate", CHALLENGE)
                        .header("dpop-nonce", "server-nonce")
                        .body(Body::empty())
                        .unwrap()
                } else {
                    Response::builder()
                        .status(200)
                        .header("content-type", "application/json")
                        .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#))
                        .unwrap()
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let uri = format!("http://{addr}/mcp");

    let mut manager = AuthorizationManager::new(uri.as_str()).await.unwrap();
    let store = InMemoryCredentialStore::new();
    store
        .save(StoredCredentials::new(
            "client".into(),
            Some(OAuthTokenResponse::new(
                AccessToken::new("bound-token".into()),
                BasicTokenType::Extension("DPoP".into()),
                VendorExtraTokenFields::default(),
            )),
            vec![],
            None,
        ))
        .await
        .unwrap();
    manager.set_credential_store(store);
    manager.set_dpop_signer(Arc::new(Es256DpopSigner::generate()));
    let client = AuthClient::new(reqwest::Client::new(), manager);

    let response = client
        .post_message(
            Arc::from(format!("{uri}?ignored=1").as_str()),
            ping(),
            None,
            None,
            HashMap::new(),
        )
        .await
        .unwrap();
    assert!(matches!(response, StreamableHttpPostResponse::Json(..)));

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    for (index, headers) in seen.iter().enumerate() {
        assert_eq!(
            headers.get_all("authorization").iter().collect::<Vec<_>>(),
            ["DPoP bound-token"],
            "the token is sent once, with the DPoP scheme"
        );
        let proof = headers.get("dpop").unwrap().to_str().unwrap();
        let claims = proof_claims(proof);
        assert_eq!(claims["htm"], "POST");
        assert_eq!(claims["htu"], uri.as_str());
        assert_eq!(claims["ath"], access_token_hash("bound-token"));
        if index == 0 {
            assert!(claims.get("nonce").is_none());
        } else {
            assert_eq!(claims["nonce"], "server-nonce");
        }
    }
}
