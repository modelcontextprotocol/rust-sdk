use std::time::Duration;

use async_trait::async_trait;

use super::error::{DiscoveryError, Result};

/// Result of fetching a manifest URL.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum FetchOutcome {
    Found { body: String },
    NotFound,
}

/// Abstraction over the HTTP calls discovery makes, so resolution can be unit
/// tested without a live server.
#[async_trait]
pub trait ManifestFetcher: Send + Sync {
    /// GET a URL. `Found` on a 2xx response, `NotFound` on 404; any other status
    /// or transport failure is an error.
    async fn get(&self, url: &str) -> Result<FetchOutcome>;

    /// Direct-handshake probe for the fallback step: returns true only if the
    /// endpoint responds like an MCP Streamable HTTP server, false otherwise.
    async fn probe(&self, url: &str) -> Result<bool>;
}

/// Minimal MCP `initialize` request used to probe an endpoint in the fallback
/// step. A non-MCP host answering on `/mcp` (a 404 page, an SPA, a login form)
/// will not produce a JSON-RPC / SSE response, so it is not mistaken for a
/// server.
const MCP_INITIALIZE_BODY: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"rmcp-discovery","version":"0"}}}"#;

/// `ManifestFetcher` backed by reqwest.
pub struct ReqwestFetcher {
    client: reqwest::Client,
}

impl ReqwestFetcher {
    /// Build a fetcher with a caller-supplied reqwest client (consistent with
    /// rmcp's OAuth custom-client pattern).
    pub fn with_client(client: reqwest::Client) -> Self {
        Self { client }
    }

    pub fn new(timeout: Duration) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(concat!("rmcp-discovery/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| DiscoveryError::Network {
                url: "<client-builder>".to_string(),
                source: e.into(),
            })?;
        Ok(Self { client })
    }
}

#[async_trait]
impl ManifestFetcher for ReqwestFetcher {
    async fn get(&self, url: &str) -> Result<FetchOutcome> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| DiscoveryError::Network {
                url: url.to_string(),
                source: e.into(),
            })?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(FetchOutcome::NotFound);
        }
        if !resp.status().is_success() {
            return Err(DiscoveryError::Network {
                url: url.to_string(),
                source: format!("unexpected status {} from {url}", resp.status()).into(),
            });
        }
        let body = resp.text().await.map_err(|e| DiscoveryError::Network {
            url: url.to_string(),
            source: e.into(),
        })?;
        Ok(FetchOutcome::Found { body })
    }

    async fn probe(&self, url: &str) -> Result<bool> {
        let resp = match self
            .client
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            )
            .body(MCP_INITIALIZE_BODY)
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) if e.is_connect() || e.is_timeout() => return Ok(false),
            Err(e) => {
                return Err(DiscoveryError::Network {
                    url: url.to_string(),
                    source: e.into(),
                });
            }
        };

        if !resp.status().is_success() {
            return Ok(false);
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();

        // An SSE response to the initialize POST is a strong MCP signal; its body
        // is a stream we must not block on reading.
        if content_type.contains("text/event-stream") {
            return Ok(true);
        }
        // For a JSON response, confirm it is actually a JSON-RPC message rather
        // than a generic API endpoint that happens to answer with JSON (e.g. a
        // catch-all returning `{"error": ...}`).
        if content_type.contains("application/json") {
            let body = resp.text().await.unwrap_or_default();
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
                return Ok(value.get("jsonrpc").is_some());
            }
        }
        Ok(false)
    }
}
