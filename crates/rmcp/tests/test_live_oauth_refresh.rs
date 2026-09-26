//! Live authorization-server checks for the refresh path.
//!
//! These are `#[ignore]`d, so they never run in CI. Provision the authorization
//! server with `./scripts/keycloak-oauth-fixture.sh`, then run them with
//! `cargo test -p rmcp --all-features --test test_live_oauth_refresh -- --ignored`.
//! `KC_BASE` overrides the server location for both the script and these tests.
#![cfg(feature = "auth")]

use std::sync::Arc;

use rmcp::transport::auth::{
    AuthError, AuthorizationManager, AuthorizationMetadata, CredentialRefreshGuard,
    CredentialStore, InMemoryCredentialStore, OAuthClientConfig, OAuthTokenResponse,
    StoredCredentials,
};
use tokio::sync::Mutex;

const REALM: &str = "rmcp";
const CLIENT_ID: &str = "rmcp-client";
const CLIENT_SECRET: &str = "rmcp-secret";

fn kc_base() -> String {
    std::env::var("KC_BASE").unwrap_or_else(|_| "http://localhost:8081".to_string())
}

fn token_endpoint() -> String {
    format!("{}/realms/{REALM}/protocol/openid-connect/token", kc_base())
}

/// Ask Keycloak for a genuine token pair through the direct access grant, so the
/// stored credentials hold a refresh token the server will actually honor.
async fn issue_real_credentials() -> OAuthTokenResponse {
    let form = format!(
        "client_id={CLIENT_ID}&client_secret={CLIENT_SECRET}\
         &username=alice&password=alice-pw&grant_type=password&scope=openid+profile"
    );
    let body = reqwest::Client::new()
        .post(token_endpoint())
        .header("content-type", "application/x-www-form-urlencoded")
        .body(form)
        .send()
        .await
        .expect("keycloak unreachable")
        .text()
        .await
        .unwrap();
    serde_json::from_str(&body).unwrap_or_else(|e| panic!("unexpected token response {body}: {e}"))
}

fn metadata() -> AuthorizationMetadata {
    // `AuthorizationMetadata` is `#[non_exhaustive]`, so fill it field by field.
    let mut metadata = AuthorizationMetadata::default();
    metadata.authorization_endpoint =
        format!("{}/realms/{REALM}/protocol/openid-connect/auth", kc_base());
    metadata.token_endpoint = token_endpoint();
    metadata
}

fn client_config() -> OAuthClientConfig {
    let mut config = OAuthClientConfig::new(CLIENT_ID, "http://localhost/callback");
    config.client_secret = Some(CLIENT_SECRET.to_string());
    config
}

async fn manager_with_store<S: CredentialStore + 'static>(store: S) -> AuthorizationManager {
    let mut manager = AuthorizationManager::new(kc_base()).await.unwrap();
    manager.set_metadata(metadata());
    manager.configure_client(client_config()).unwrap();
    manager.set_credential_store(store);
    manager
}

fn stored(client_id: &str, token: OAuthTokenResponse) -> StoredCredentials {
    StoredCredentials::new(
        client_id.to_string(),
        Some(token),
        vec!["openid".into(), "profile".into()],
        None,
    )
}

/// A store that serializes refreshes the way a shared on-disk store would.
#[derive(Clone, Default)]
struct GuardedStore {
    inner: InMemoryCredentialStore,
    lock: Arc<Mutex<()>>,
}

#[async_trait::async_trait]
impl CredentialStore for GuardedStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        self.inner.load().await
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        self.inner.save(credentials).await
    }

    async fn clear(&self) -> Result<(), AuthError> {
        self.inner.clear().await
    }

    async fn acquire_refresh_guard(&self) -> Result<Option<CredentialRefreshGuard>, AuthError> {
        Ok(Some(CredentialRefreshGuard::new(
            self.lock.clone().lock_owned().await,
        )))
    }
}

#[tokio::test]
#[ignore = "requires a live Keycloak"]
async fn live_refresh_rotates_the_stored_token() {
    use oauth2::TokenResponse;

    let issued = issue_real_credentials().await;
    let original_refresh = issued.refresh_token().unwrap().secret().clone();
    let store = InMemoryCredentialStore::new();
    store.save(stored(CLIENT_ID, issued)).await.unwrap();
    let manager = manager_with_store(store.clone()).await;

    let refreshed = manager.refresh_token().await.expect("live refresh failed");

    let saved = store.load().await.unwrap().unwrap();
    let saved_token = saved.token_response.unwrap();
    assert_ne!(
        saved_token.refresh_token().unwrap().secret(),
        &original_refresh,
        "keycloak rotates refresh tokens, so the store must hold the new one"
    );
    assert_eq!(
        saved_token.access_token().secret(),
        refreshed.access_token().secret(),
        "the saved credentials must match what the caller received"
    );
    assert!(
        saved.granted_scopes.contains(&"openid".to_string()),
        "granted scopes should come from the provider response, got: {:?}",
        saved.granted_scopes
    );
}

#[tokio::test]
#[ignore = "requires a live Keycloak"]
async fn live_refresh_rejects_credentials_for_another_client() {
    use oauth2::TokenResponse;

    let issued = issue_real_credentials().await;
    let untouched_refresh = issued.refresh_token().unwrap().secret().clone();
    let store = InMemoryCredentialStore::new();
    store
        .save(stored("some-other-client", issued))
        .await
        .unwrap();
    let manager = manager_with_store(store.clone()).await;

    let error = manager.refresh_token().await.unwrap_err();

    assert!(
        matches!(error, AuthError::AuthorizationRequired),
        "a client mismatch must require reauthorization, got: {error:?}"
    );
    let saved = store.load().await.unwrap().unwrap();
    assert_eq!(
        saved
            .token_response
            .unwrap()
            .refresh_token()
            .unwrap()
            .secret(),
        &untouched_refresh,
        "a rejected refresh must leave the stored token untouched"
    );
}

#[tokio::test]
#[ignore = "requires a live Keycloak"]
async fn live_concurrent_refreshes_survive_refresh_token_rotation() {
    use oauth2::TokenResponse;

    let issued = issue_real_credentials().await;
    let store = GuardedStore::default();
    store.save(stored(CLIENT_ID, issued)).await.unwrap();
    let first = manager_with_store(store.clone()).await;
    let second = manager_with_store(store.clone()).await;

    // Without the guard the second caller would reuse the refresh token the first
    // one already consumed, and Keycloak would answer `invalid_grant`.
    let (a, b) = tokio::join!(
        tokio::spawn(async move { first.refresh_token().await }),
        tokio::spawn(async move { second.refresh_token().await })
    );

    let a = a.unwrap().expect("first concurrent refresh failed");
    let b = b.unwrap().expect("second concurrent refresh failed");
    assert_ne!(
        a.access_token().secret(),
        b.access_token().secret(),
        "each caller performs its own exchange, so the tokens must differ"
    );
}

/// Deterministic proof that this realm really does invalidate a rotated refresh
/// token, which is what makes the coordination above load-bearing.
#[tokio::test]
#[ignore = "requires a live Keycloak with revokeRefreshToken enabled"]
async fn live_reusing_a_rotated_refresh_token_is_rejected() {
    let issued = issue_real_credentials().await;

    let first_store = InMemoryCredentialStore::new();
    first_store
        .save(stored(CLIENT_ID, issued.clone()))
        .await
        .unwrap();
    manager_with_store(first_store)
        .await
        .refresh_token()
        .await
        .expect("the first refresh should succeed");

    // A second caller that never saw the rotation still holds the consumed token.
    let stale_store = InMemoryCredentialStore::new();
    stale_store.save(stored(CLIENT_ID, issued)).await.unwrap();
    let error = manager_with_store(stale_store)
        .await
        .refresh_token()
        .await
        .unwrap_err();

    assert!(
        matches!(error, AuthError::TokenRefreshRejected(_)),
        "reusing a rotated refresh token must be rejected, got: {error:?}"
    );
}
