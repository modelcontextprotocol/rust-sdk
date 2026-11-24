//! DPoP (RFC 9449) client support for the SEP-1932 DPoP profile.
//!
//! A [`DpopSigner`] holds the proof key. The SDK builds the proof JWT and asks
//! the signer only for a signature, so the key can live outside the process
//! (an HSM, a platform keystore, or a remote signer). [`Es256DpopSigner`] is
//! the in-process default.
//!
//! Set a signer with
//! [`AuthorizationManager::set_dpop_signer`](super::AuthorizationManager::set_dpop_signer).
//! The SDK then builds proofs for token requests and resource requests, and
//! remembers the most recent `DPoP-Nonce` from each server.

use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use p256::ecdsa::{Signature, SigningKey, signature::Signer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

/// The `typ` header value of a DPoP proof (RFC 9449 §4.2).
const DPOP_PROOF_TYP: &str = "dpop+jwt";

/// The HTTP header that carries a DPoP proof.
pub(crate) const DPOP_HEADER: &str = "DPoP";

/// The `Authorization` scheme for DPoP-bound access tokens.
pub(crate) const DPOP_AUTH_SCHEME: &str = "DPoP";

/// The error code a server returns when a proof must carry its nonce.
const USE_DPOP_NONCE_ERROR: &str = "use_dpop_nonce";

/// A JWS algorithm that a [`DpopSigner`] can sign with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DpopAlgorithm {
    /// ECDSA with P-256 and SHA-256. The signature is the 64-byte `r || s` form.
    Es256,
}

impl DpopAlgorithm {
    /// The JWS `alg` header value.
    pub fn as_str(&self) -> &'static str {
        match self {
            DpopAlgorithm::Es256 => "ES256",
        }
    }
}

impl fmt::Display for DpopAlgorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The public half of a DPoP key, as a JWK.
///
/// Only the required members are kept, so the value serializes to the exact
/// form placed in the proof header and hashed for the RFC 7638 thumbprint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kty")]
#[non_exhaustive]
pub enum PublicJwk {
    /// An elliptic-curve public key.
    #[serde(rename = "EC")]
    Ec {
        /// The curve name, such as `P-256`.
        crv: String,
        /// The base64url x coordinate.
        x: String,
        /// The base64url y coordinate.
        y: String,
    },
}

impl PublicJwk {
    /// The RFC 7638 JWK SHA-256 thumbprint, base64url-encoded.
    ///
    /// This is the value an authorization server puts in the access token's
    /// `cnf.jkt` claim and accepts as the `dpop_jkt` authorization parameter.
    pub fn thumbprint(&self) -> String {
        // RFC 7638 §3.2: the required members only, in lexicographic order,
        // with no whitespace. Written out by hand so the result does not
        // depend on serde_json's map ordering.
        let bytes = match self {
            PublicJwk::Ec { crv, x, y } => format!(
                r#"{{"crv":{},"kty":"EC","x":{},"y":{}}}"#,
                serde_json::Value::from(crv.as_str()),
                serde_json::Value::from(x.as_str()),
                serde_json::Value::from(y.as_str()),
            ),
        };
        URL_SAFE_NO_PAD.encode(Sha256::digest(bytes))
    }
}

/// An error from a [`DpopSigner`].
#[derive(Debug, thiserror::Error)]
#[error("DPoP signing failed: {message}")]
pub struct DpopSignError {
    message: String,
    #[source]
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl DpopSignError {
    /// Create an error with a message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source: None,
        }
    }

    /// Create an error with a message and an underlying cause.
    pub fn with_source(
        message: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }
}

/// Signs DPoP proofs.
///
/// The SDK builds the JWS signing input (`base64url(header) "."
/// base64url(claims)`) and passes it to [`sign`](Self::sign). The signer
/// returns the raw JWS signature for [`algorithm`](Self::algorithm). For
/// `ES256` that is the 64-byte `r || s` form, not DER.
///
/// The key must stay the same for the lifetime of the access tokens bound to
/// it: the authorization server binds each token to the key's thumbprint.
#[async_trait]
pub trait DpopSigner: Send + Sync {
    /// The JWS algorithm this signer uses.
    fn algorithm(&self) -> DpopAlgorithm;

    /// The public key placed in the proof's `jwk` header.
    fn public_jwk(&self) -> &PublicJwk;

    /// Sign the JWS signing input.
    async fn sign(&self, signing_input: &[u8]) -> Result<Vec<u8>, DpopSignError>;
}

/// An in-process ES256 DPoP signer.
///
/// [`generate`](Self::generate) creates a new key. The key is not persisted:
/// tokens bound to it become unusable when the process exits, so pair it with
/// a credential store that does not outlive the process, or implement
/// [`DpopSigner`] over a persistent key.
#[derive(Clone)]
pub struct Es256DpopSigner {
    key: SigningKey,
    jwk: PublicJwk,
}

impl fmt::Debug for Es256DpopSigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Es256DpopSigner")
            .field("jwk", &self.jwk)
            .finish_non_exhaustive()
    }
}

impl Es256DpopSigner {
    /// Generate a new P-256 key from the operating system's random source.
    pub fn generate() -> Self {
        Self::from_signing_key(SigningKey::random(&mut rand_core::OsRng))
    }

    /// Load a P-256 private key from its 32-byte big-endian scalar.
    pub fn from_secret_bytes(secret: &[u8]) -> Result<Self, DpopSignError> {
        let key = SigningKey::from_slice(secret)
            .map_err(|error| DpopSignError::with_source("invalid P-256 private key", error))?;
        Ok(Self::from_signing_key(key))
    }

    fn from_signing_key(key: SigningKey) -> Self {
        let point = key.verifying_key().to_encoded_point(false);
        let jwk = PublicJwk::Ec {
            crv: "P-256".to_string(),
            x: URL_SAFE_NO_PAD.encode(point.x().expect("uncompressed point has x")),
            y: URL_SAFE_NO_PAD.encode(point.y().expect("uncompressed point has y")),
        };
        Self { key, jwk }
    }
}

#[async_trait]
impl DpopSigner for Es256DpopSigner {
    fn algorithm(&self) -> DpopAlgorithm {
        DpopAlgorithm::Es256
    }

    fn public_jwk(&self) -> &PublicJwk {
        &self.jwk
    }

    async fn sign(&self, signing_input: &[u8]) -> Result<Vec<u8>, DpopSignError> {
        let signature: Signature = self.key.sign(signing_input);
        Ok(signature.to_bytes().to_vec())
    }
}

#[derive(Serialize)]
struct ProofHeader<'a> {
    typ: &'static str,
    alg: &'static str,
    jwk: &'a PublicJwk,
}

#[derive(Serialize)]
struct ProofClaims<'a> {
    jti: String,
    htm: &'a str,
    htu: &'a str,
    iat: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    ath: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nonce: Option<&'a str>,
}

/// Builds DPoP proofs and tracks server nonces.
///
/// Nonces are kept per origin, because the authorization server and the
/// resource server issue their own (RFC 9449 §8 and §9).
#[derive(Clone)]
pub(crate) struct DpopProofFactory {
    signer: Arc<dyn DpopSigner>,
    nonces: Arc<Mutex<HashMap<String, String>>>,
}

impl fmt::Debug for DpopProofFactory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DpopProofFactory")
            .field("algorithm", &self.signer.algorithm())
            .field("jwk", self.signer.public_jwk())
            .finish_non_exhaustive()
    }
}

/// An error building a DPoP proof.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub(crate) enum DpopProofError {
    /// The request URI cannot be used as `htu`.
    #[error("invalid DPoP target URI `{0}`")]
    InvalidUri(String),
    /// The signer failed.
    #[error(transparent)]
    Sign(#[from] DpopSignError),
}

impl DpopProofFactory {
    /// Create a factory around a signer.
    pub(crate) fn new(signer: Arc<dyn DpopSigner>) -> Self {
        Self {
            signer,
            nonces: Arc::default(),
        }
    }

    /// The signer's public key.
    #[cfg(test)]
    fn public_jwk(&self) -> &PublicJwk {
        self.signer.public_jwk()
    }

    /// Build a proof for `method` and `uri`.
    ///
    /// Pass `access_token` for a resource request, so the proof carries its
    /// `ath` hash. Pass `None` for a token request. The proof includes the
    /// last nonce recorded for the URI's origin, if any.
    #[cfg(test)]
    async fn proof(
        &self,
        method: &str,
        uri: &str,
        access_token: Option<&str>,
    ) -> Result<String, DpopProofError> {
        self.proof_and_nonce(method, uri, access_token)
            .await
            .map(|(proof, _)| proof)
    }

    /// Like [`proof`](Self::proof), and also returns the nonce the proof
    /// carries, so a caller can tell whether a server's new nonce differs from
    /// the one it just sent.
    pub(crate) async fn proof_and_nonce(
        &self,
        method: &str,
        uri: &str,
        access_token: Option<&str>,
    ) -> Result<(String, Option<String>), DpopProofError> {
        self.proof_at(method, uri, access_token, now_epoch_secs())
            .await
    }

    async fn proof_at(
        &self,
        method: &str,
        uri: &str,
        access_token: Option<&str>,
        iat: u64,
    ) -> Result<(String, Option<String>), DpopProofError> {
        let (htu, origin) = htu_and_origin(uri)?;
        let nonce = self.nonce_for_origin(&origin);
        let header = ProofHeader {
            typ: DPOP_PROOF_TYP,
            alg: self.signer.algorithm().as_str(),
            jwk: self.signer.public_jwk(),
        };
        let claims = ProofClaims {
            jti: uuid::Uuid::new_v4().to_string(),
            htm: method,
            htu: &htu,
            iat,
            ath: access_token.map(access_token_hash),
            nonce: nonce.as_deref(),
        };
        let signing_input = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).expect("header serializes")),
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).expect("claims serialize")),
        );
        let signature = self.signer.sign(signing_input.as_bytes()).await?;
        if self.signer.algorithm() == DpopAlgorithm::Es256 && signature.len() != ES256_SIGNATURE_LEN
        {
            return Err(DpopSignError::new(format!(
                "ES256 signature must be {ES256_SIGNATURE_LEN} bytes (r || s), got {}; DER is not accepted",
                signature.len()
            ))
            .into());
        }
        Ok((
            format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(signature)),
            nonce,
        ))
    }

    /// Record a server's `DPoP-Nonce` and say whether a retry is worthwhile:
    /// the server's nonce must differ from `sent`, the nonce the failed
    /// request carried. Comparing with the sent nonce, not the stored one,
    /// keeps a concurrent request that already stored it from losing its retry.
    pub(crate) fn record_nonce_for_retry(
        &self,
        uri: &str,
        sent: Option<&str>,
        server_nonce: &str,
    ) -> bool {
        self.record_nonce(uri, server_nonce);
        sent != Some(server_nonce)
    }

    /// Record a `DPoP-Nonce` value a server sent for `uri`, so the next proof
    /// for that origin carries it.
    ///
    /// Returns `true` when this changed the recorded nonce, and `false` when
    /// the same nonce was already recorded or `uri` is not usable. This is not
    /// a retry decision: [`record_nonce_for_retry`](Self::record_nonce_for_retry)
    /// compares with the nonce the failed request sent.
    pub(crate) fn record_nonce(&self, uri: &str, nonce: &str) -> bool {
        let Ok((_, origin)) = htu_and_origin(uri) else {
            return false;
        };
        let mut nonces = self.nonces.lock().unwrap_or_else(|e| e.into_inner());
        match nonces.get(&origin) {
            Some(existing) if existing == nonce => false,
            _ => {
                nonces.insert(origin, nonce.to_string());
                true
            }
        }
    }

    fn nonce_for_origin(&self, origin: &str) -> Option<String> {
        self.nonces
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(origin)
            .cloned()
    }
}

const ES256_SIGNATURE_LEN: usize = 64;

/// The `ath` claim: base64url SHA-256 of the access token (RFC 9449 §4.2).
pub(crate) fn access_token_hash(access_token: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(access_token.as_bytes()))
}

/// `htu` is the target URI without query and fragment (RFC 9449 §4.2).
fn htu_and_origin(uri: &str) -> Result<(String, String), DpopProofError> {
    let mut url = Url::parse(uri).map_err(|_| DpopProofError::InvalidUri(uri.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
        return Err(DpopProofError::InvalidUri(uri.to_string()));
    }
    url.set_query(None);
    url.set_fragment(None);
    // Userinfo is not part of the request target.
    let _ = url.set_username("");
    let _ = url.set_password(None);
    let origin = url.origin().ascii_serialization();
    Ok((url.to_string(), origin))
}

fn now_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// Whether an OAuth `token_type` names a DPoP-bound token (RFC 9449 §5).
pub(crate) fn is_dpop_token_type(token_type: &str) -> bool {
    token_type.eq_ignore_ascii_case(DPOP_AUTH_SCHEME)
}

/// Whether an OAuth error response body is `use_dpop_nonce` (RFC 9449 §8).
pub(crate) fn is_use_dpop_nonce_error(body: &[u8]) -> bool {
    #[derive(Deserialize)]
    struct ErrorBody {
        error: String,
    }
    serde_json::from_slice::<ErrorBody>(body).is_ok_and(|body| body.error == USE_DPOP_NONCE_ERROR)
}

#[cfg(test)]
mod tests {
    use p256::ecdsa::{VerifyingKey, signature::Verifier};

    use super::*;

    fn decode_segment(segment: &str) -> serde_json::Value {
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(segment).unwrap()).unwrap()
    }

    fn verifying_key(jwk: &PublicJwk) -> VerifyingKey {
        let PublicJwk::Ec { x, y, .. } = jwk;
        let mut sec1 = vec![0x04];
        sec1.extend(URL_SAFE_NO_PAD.decode(x).unwrap());
        sec1.extend(URL_SAFE_NO_PAD.decode(y).unwrap());
        VerifyingKey::from_sec1_bytes(&sec1).unwrap()
    }

    #[test]
    fn thumbprint_matches_rfc_9449_example() {
        // The proof key and `cnf.jkt` from RFC 9449 §4.1 and §6.1.
        let jwk = PublicJwk::Ec {
            crv: "P-256".to_string(),
            x: "l8tFrhx-34tV3hRICRDY9zCkDlpBhF42UQUfWVAWBFs".to_string(),
            y: "9VE4jf_Ok_o64zbTTlcuNJajHmt6v9TDVrU0CdvGRDA".to_string(),
        };
        assert_eq!(
            jwk.thumbprint(),
            "0ZcOCORZNYy-DWpqq30jZyJGHTN0d2HglBV3uiguA4I"
        );
    }

    #[test]
    fn access_token_hash_matches_rfc_9449_example() {
        // The access token and `ath` from RFC 9449 §7.1.
        assert_eq!(
            access_token_hash("Kz~8mXK1EalYznwH-LC-1fBAo.4Ljp~zsPE_NeO.gxU"),
            "fUHyO2r2Z3DZ53EsNrWBb0xWXoaNy59IiKCAqksmQEo"
        );
    }

    #[tokio::test]
    async fn proof_has_required_header_and_claims_and_verifies() {
        let factory = DpopProofFactory::new(Arc::new(Es256DpopSigner::generate()));
        let proof = factory
            .proof_at(
                "POST",
                // `htu` drops userinfo, query and fragment (RFC 9449 §4.2).
                "https://u:p@mcp.example.com/mcp?x=1#frag",
                Some("token"),
                1_700_000_000,
            )
            .await
            .unwrap()
            .0;
        let segments: Vec<&str> = proof.split('.').collect();
        assert_eq!(segments.len(), 3);

        let header = decode_segment(segments[0]);
        assert_eq!(header["typ"], "dpop+jwt");
        assert_eq!(header["alg"], "ES256");
        assert_eq!(
            header["jwk"],
            serde_json::to_value(factory.public_jwk()).unwrap()
        );
        assert!(header["jwk"].get("d").is_none());
        let mut header_keys: Vec<&str> = header
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        header_keys.sort_unstable();
        assert_eq!(header_keys, ["alg", "jwk", "typ"]);

        let claims = decode_segment(segments[1]);
        assert_eq!(claims["htm"], "POST");
        assert_eq!(claims["htu"], "https://mcp.example.com/mcp");
        assert_eq!(claims["iat"], 1_700_000_000);
        assert_eq!(claims["ath"], access_token_hash("token"));
        assert!(claims["jti"].as_str().is_some_and(|jti| !jti.is_empty()));
        let mut claim_keys: Vec<&str> = claims
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        claim_keys.sort_unstable();
        assert_eq!(claim_keys, ["ath", "htm", "htu", "iat", "jti"]);

        let signature =
            Signature::from_slice(&URL_SAFE_NO_PAD.decode(segments[2]).unwrap()).unwrap();
        verifying_key(factory.public_jwk())
            .verify(
                format!("{}.{}", segments[0], segments[1]).as_bytes(),
                &signature,
            )
            .unwrap();
    }

    #[tokio::test]
    async fn nonces_are_kept_per_origin() {
        let factory = DpopProofFactory::new(Arc::new(Es256DpopSigner::generate()));
        assert!(factory.record_nonce("https://as.example.com/token", "as-nonce"));
        assert!(!factory.record_nonce("https://as.example.com/other", "as-nonce"));
        assert!(factory.record_nonce("https://mcp.example.com/mcp", "rs-nonce"));

        let as_proof = factory
            .proof("POST", "https://as.example.com/token", None)
            .await
            .unwrap();
        let rs_proof = factory
            .proof("POST", "https://mcp.example.com/mcp", Some("t"))
            .await
            .unwrap();
        let other_proof = factory
            .proof("POST", "https://other.example.com/mcp", Some("t"))
            .await
            .unwrap();
        assert_eq!(
            decode_segment(as_proof.split('.').nth(1).unwrap())["nonce"],
            "as-nonce"
        );
        assert_eq!(
            decode_segment(rs_proof.split('.').nth(1).unwrap())["nonce"],
            "rs-nonce"
        );
        assert!(
            decode_segment(other_proof.split('.').nth(1).unwrap())
                .get("nonce")
                .is_none()
        );
    }

    #[tokio::test]
    async fn rejects_non_http_uri() {
        let factory = DpopProofFactory::new(Arc::new(Es256DpopSigner::generate()));
        assert!(matches!(
            factory.proof("GET", "urn:example", None).await,
            Err(DpopProofError::InvalidUri(_))
        ));
    }

    struct DerSigner {
        jwk: PublicJwk,
    }

    #[async_trait]
    impl DpopSigner for DerSigner {
        fn algorithm(&self) -> DpopAlgorithm {
            DpopAlgorithm::Es256
        }

        fn public_jwk(&self) -> &PublicJwk {
            &self.jwk
        }

        async fn sign(&self, _signing_input: &[u8]) -> Result<Vec<u8>, DpopSignError> {
            Ok(vec![0x30; 70])
        }
    }

    #[tokio::test]
    async fn rejects_es256_signature_that_is_not_64_bytes() {
        let jwk = Es256DpopSigner::generate().public_jwk().clone();
        let factory = DpopProofFactory::new(Arc::new(DerSigner { jwk }));
        let error = factory
            .proof("POST", "https://as.example.com/token", None)
            .await
            .unwrap_err();
        assert!(matches!(error, DpopProofError::Sign(_)));
        assert!(error.to_string().contains("64 bytes"));
    }

    #[test]
    fn secret_bytes_derive_the_expected_public_jwk() {
        // Scalar 1 has the P-256 generator as its public point, so the
        // expected coordinates are fixed by the curve definition.
        let mut secret = [0u8; 32];
        secret[31] = 1;
        let signer = Es256DpopSigner::from_secret_bytes(&secret).unwrap();
        assert_eq!(
            serde_json::to_value(signer.public_jwk()).unwrap(),
            serde_json::json!({
                "kty": "EC",
                "crv": "P-256",
                "x": "axfR8uEsQkf4vOblY6RA8ncDfYEt6zOg9KE5RdiYwpY",
                "y": "T-NC4v4af5uO5-tKfA-eFivOM1drMV7Oy7ZAaDe_UfU",
            })
        );
        assert!(Es256DpopSigner::from_secret_bytes(&[0u8; 32]).is_err());
    }
}
