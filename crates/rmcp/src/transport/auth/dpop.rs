//! DPoP (RFC 9449) client support for the SEP-1932 DPoP profile.
//!
//! A [`DpopSigner`] holds the proof key. The SDK builds the proof JWT and asks
//! the signer only for a signature, so the key can live outside the process
//! (an HSM, a platform keystore, or a remote signer). [`Es256DpopSigner`] is
//! the in-process default.
//!
//! This module holds the signing key types. Building proofs and sending them
//! on token and resource requests is a separate change.

use std::fmt;

use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use p256::ecdsa::{Signature, SigningKey, signature::Signer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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
#[cfg(test)]
mod tests {
    use p256::ecdsa::{VerifyingKey, signature::Verifier};

    use super::*;

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

    #[tokio::test]
    async fn es256_signature_is_64_bytes_and_verifies_with_the_public_jwk() {
        let signer = Es256DpopSigner::generate();
        let input = b"header.claims";
        let signature = signer.sign(input).await.unwrap();
        assert_eq!(signature.len(), 64, "ES256 uses r || s, not DER");
        let signature = Signature::from_slice(&signature).unwrap();
        verifying_key(signer.public_jwk())
            .verify(input, &signature)
            .unwrap();
        assert_eq!(signer.algorithm().as_str(), "ES256");
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
