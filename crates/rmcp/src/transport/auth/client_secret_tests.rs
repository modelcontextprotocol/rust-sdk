use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

#[test]
fn legacy_credentials_remain_public_and_debug_redacts_secrets() {
    let mut stored: StoredCredentials = serde_json::from_value(serde_json::json!({
        "client_id": "public", "token_response": null
    }))
    .unwrap();
    assert!(stored.client_secret.is_none());
    stored = stored.with_client_secret(Some(ClientSecret::new("synthetic-secret".into())));
    let encoded = serde_json::to_string(&stored).unwrap();
    let restored: StoredCredentials = serde_json::from_str(&encoded).unwrap();
    assert_eq!(restored.client_secret.unwrap().secret(), "synthetic-secret");
    assert!(!format!("{stored:?}").contains("synthetic-secret"));
    assert!(
        stored
            .with_client_secret(Some(ClientSecret::new(String::new())))
            .client_secret
            .is_none()
    );
}

#[tokio::test]
async fn issuer_change_discards_the_registered_secret_before_restoration() {
    for client_id in ["registered-client", "https://client.example/metadata.json"] {
        let mut manager = AuthorizationManager::new("https://resource.example")
            .await
            .unwrap();
        manager.set_metadata(AuthorizationMetadata {
            authorization_endpoint: "https://new.example/authorize".into(),
            token_endpoint: "https://new.example/token".into(),
            issuer: Some("https://new.example".into()),
            ..Default::default()
        });
        let stored = serde_json::from_value(serde_json::json!({
            "client_id": client_id, "client_secret": "synthetic-secret",
            "issuer": "https://old.example",
            "token_response": {"access_token":"old-access", "token_type":"Bearer"}
        }))
        .unwrap();
        manager.credential_store.save(stored).await.unwrap();
        assert!(!manager.initialize_from_store().await.unwrap());
        assert!(manager.client_secret().is_none());
        if let Some(saved) = manager.credential_store.load().await.unwrap() {
            assert!(saved.client_secret.is_none());
            assert!(saved.token_response.is_none());
        }
    }
}

#[tokio::test]
async fn restored_clients_authenticate_refresh_and_keep_rotating_credentials() {
    use std::collections::HashMap;

    use axum::{Router, body::Bytes, http::HeaderMap, routing::post};
    use base64::Engine;

    for method in ["client_secret_basic", "client_secret_post", "none"] {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let app = Router::new().route(
            "/token",
            post(move |headers: HeaderMap, body: Bytes| {
                let calls = observed.clone();
                async move {
                    let form: HashMap<_, _> =
                        url::form_urlencoded::parse(&body).into_owned().collect();
                    let count = calls.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(form["grant_type"], "refresh_token");
                    assert_eq!(
                        form["refresh_token"],
                        if count == 0 {
                            "initial-refresh"
                        } else {
                            "rotated-refresh"
                        }
                    );
                    assert!(form["resource"].starts_with("http://127.0.0.1:"));
                    match method {
                        "client_secret_basic" => {
                            let value = base64::engine::general_purpose::STANDARD
                                .encode("test-client:synthetic-secret");
                            assert_eq!(headers["authorization"], format!("Basic {value}"));
                            assert!(!form.contains_key("client_secret"));
                        }
                        "client_secret_post" => {
                            assert_eq!(form["client_id"], "test-client");
                            assert_eq!(form["client_secret"], "synthetic-secret");
                            assert!(!headers.contains_key("authorization"));
                        }
                        _ => {
                            assert_eq!(form["client_id"], "test-client");
                            assert!(!form.contains_key("client_secret"));
                            assert!(!headers.contains_key("authorization"));
                        }
                    }
                    let mut response = serde_json::json!({
                        "access_token": "new-access", "token_type": "Bearer", "expires_in": 3600
                    });
                    if count == 0 {
                        response["refresh_token"] = "rotated-refresh".into();
                    }
                    axum::http::Response::builder()
                        .header("content-type", "application/json")
                        .body(axum::body::Body::from(response.to_string()))
                        .unwrap()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut stored: StoredCredentials = serde_json::from_value(serde_json::json!({
            "client_id": "test-client",
            "token_response": {"access_token":"old-access", "token_type":"Bearer", "expires_in":1, "refresh_token":"initial-refresh"},
            "token_received_at": 0,
            "issuer": base
        })).unwrap();
        if method != "none" {
            stored = stored.with_client_secret(Some(ClientSecret::new("synthetic-secret".into())));
        }
        for _ in 0..2 {
            // Only serialized credentials survive between manager instances.
            let encoded = serde_json::to_vec(&stored).unwrap();
            let mut manager = AuthorizationManager::new(&base).await.unwrap();
            let mut metadata = AuthorizationMetadata {
                authorization_endpoint: format!("{base}/authorize"),
                token_endpoint: format!("{base}/token"),
                issuer: Some(base.clone()),
                ..Default::default()
            };
            metadata.additional_fields.insert(
                "token_endpoint_auth_methods_supported".into(),
                serde_json::json!([method]),
            );
            manager.set_metadata(metadata);
            manager
                .credential_store
                .save(serde_json::from_slice(&encoded).unwrap())
                .await
                .unwrap();
            assert!(manager.initialize_from_store().await.unwrap());
            manager.refresh_token().await.unwrap();
            stored = manager.credential_store.load().await.unwrap().unwrap();
            assert_eq!(
                stored
                    .client_secret
                    .as_ref()
                    .map(|secret| secret.secret().as_str()),
                if method == "none" {
                    None
                } else {
                    Some("synthetic-secret")
                }
            );
            assert_eq!(
                stored
                    .token_response
                    .as_ref()
                    .unwrap()
                    .refresh_token()
                    .unwrap()
                    .secret(),
                "rotated-refresh"
            );
            assert!(!format!("{stored:?}").contains("synthetic-secret"));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        server.abort();
    }
}
