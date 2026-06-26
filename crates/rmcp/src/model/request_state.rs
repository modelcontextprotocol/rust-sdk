//! Integrity protection for SEP-2322 `requestState`.
//!
//! In the multi round-trip request (MRTR) flow, a server places an opaque
//! `requestState` string in an [`InputRequiredResult`](super::InputRequiredResult)
//! and the client echoes it back verbatim on retry. From the server's point of
//! view the echoed value is **untrusted, attacker-controlled input**: a client
//! can send back anything it likes. A server that stores meaningful state inside
//! `requestState` (rather than in a server-side session) MUST verify that the
//! value it receives is one it actually produced.
//!
//! [`RequestStateCodec`] provides an opt-in way to do this. It seals a payload
//! into an opaque string with an HMAC-SHA256 tag and opens it again, rejecting
//! any value that was forged or tampered with. This mirrors the approach taken
//! by other MCP SDKs for stateless MRTR servers.
//!
//! This helper is only about *integrity*, not *confidentiality*: the sealed
//! payload is signed, not encrypted, so it is base64url-readable by anyone.
//! Do not put secrets in it. Replay protection (nonces, expiry) is the caller's
//! responsibility and can be embedded in the sealed payload.
//!
//! Using the codec is entirely optional. A server that keeps its state
//! server-side, or that does not trust `requestState` for anything security
//! sensitive, can keep building the string by hand via
//! [`InputRequiredResult::from_request_state`](super::InputRequiredResult::from_request_state).
//!
//! # Examples
//!
//! ```
//! use rmcp::model::RequestStateCodec;
//!
//! // Derive the key from a per-process secret; keep it out of client reach.
//! let codec = RequestStateCodec::new(b"a-32-byte-or-longer-secret-key!!!");
//!
//! let sealed = codec.seal(b"tool=weather;step=2");
//! // `sealed` is what the server puts in `InputRequiredResult::request_state`.
//!
//! // On retry the client echoes `sealed` back untouched.
//! let opened = codec.open(&sealed).expect("integrity check passes");
//! assert_eq!(opened, b"tool=weather;step=2");
//!
//! // A tampered value is rejected instead of silently trusted.
//! let mut tampered = sealed.clone();
//! tampered.push('x');
//! assert!(codec.open(&tampered).is_err());
//! ```

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use serde::{Serialize, de::DeserializeOwned};
use sha2::Sha256;
use thiserror::Error;

type HmacSha256 = Hmac<Sha256>;

/// Version tag prefixing every sealed value, so the wire format can evolve.
const VERSION: &str = "rs1";

/// Domain-separation label mixed into the HMAC so a `requestState` tag can never
/// be confused with an HMAC computed for some other purpose using the same key.
const DOMAIN: &[u8] = b"rmcp/mrtr/request-state/v1";

/// Errors returned when opening a sealed [`RequestStateCodec`] value.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RequestStateError {
    /// The value is not a well-formed sealed request state (wrong prefix or
    /// missing sections).
    #[error("request state is malformed or uses an unsupported format")]
    MalformedFormat,

    /// A section of the value was not valid base64url.
    #[error("request state is not valid base64url")]
    InvalidEncoding,

    /// The HMAC tag did not match; the value was forged or tampered with.
    #[error("request state failed integrity verification")]
    IntegrityCheckFailed,

    /// The sealed payload could not be serialized to JSON.
    #[error("failed to serialize request state payload: {0}")]
    Serialization(#[source] serde_json::Error),

    /// The opened payload could not be deserialized from JSON.
    #[error("failed to deserialize request state payload: {0}")]
    Deserialization(#[source] serde_json::Error),
}

/// A keyed codec that seals and opens SEP-2322 `requestState` values with
/// HMAC-SHA256 integrity protection.
///
/// Construct one codec per signing key and reuse it for the lifetime of the
/// key. The same key must be used to [`seal`](Self::seal) and
/// [`open`](Self::open) a value, so it has to survive across the rounds of a
/// single MRTR exchange (e.g. a stable per-process or per-deployment secret).
///
/// The key may be any length; HMAC internally normalizes it. For meaningful
/// security use a high-entropy key of at least 32 bytes.
#[derive(Clone)]
pub struct RequestStateCodec {
    key: Box<[u8]>,
}

impl std::fmt::Debug for RequestStateCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never leak the signing key through Debug output.
        f.debug_struct("RequestStateCodec")
            .field("key", &"<redacted>")
            .finish()
    }
}

impl RequestStateCodec {
    /// Creates a codec from a signing key.
    pub fn new(key: impl Into<Vec<u8>>) -> Self {
        Self {
            key: key.into().into_boxed_slice(),
        }
    }

    /// Seals raw bytes into an opaque, integrity-protected string suitable for
    /// use as `requestState`.
    pub fn seal(&self, payload: &[u8]) -> String {
        let mut mac = self.mac();
        mac.update(payload);
        let tag = mac.finalize().into_bytes();

        // base64url without padding encodes 3 bytes as 4 chars, rounding up.
        let b64_len = |n: usize| n.div_ceil(3) * 4;
        let mut out =
            String::with_capacity(VERSION.len() + 2 + b64_len(payload.len()) + b64_len(tag.len()));
        out.push_str(VERSION);
        out.push('.');
        URL_SAFE_NO_PAD.encode_string(payload, &mut out);
        out.push('.');
        URL_SAFE_NO_PAD.encode_string(tag.as_slice(), &mut out);
        out
    }

    /// Seals a serializable value by encoding it as JSON before sealing.
    ///
    /// # Errors
    ///
    /// Returns [`RequestStateError::Serialization`] if `value` cannot be encoded
    /// as JSON.
    pub fn seal_json<T: Serialize>(&self, value: &T) -> Result<String, RequestStateError> {
        let payload = serde_json::to_vec(value).map_err(RequestStateError::Serialization)?;
        Ok(self.seal(&payload))
    }

    /// Opens a sealed value, verifying its integrity and returning the original
    /// bytes.
    ///
    /// # Errors
    ///
    /// Returns [`RequestStateError::IntegrityCheckFailed`] if the value was not
    /// produced by this key, and [`RequestStateError::MalformedFormat`] or
    /// [`RequestStateError::InvalidEncoding`] if it is not a well-formed sealed
    /// value.
    pub fn open(&self, sealed: &str) -> Result<Vec<u8>, RequestStateError> {
        let mut parts = sealed.split('.');
        let version = parts.next().ok_or(RequestStateError::MalformedFormat)?;
        let payload_b64 = parts.next().ok_or(RequestStateError::MalformedFormat)?;
        let tag_b64 = parts.next().ok_or(RequestStateError::MalformedFormat)?;
        if parts.next().is_some() || version != VERSION {
            return Err(RequestStateError::MalformedFormat);
        }

        let payload = URL_SAFE_NO_PAD
            .decode(payload_b64)
            .map_err(|_| RequestStateError::InvalidEncoding)?;
        let tag = URL_SAFE_NO_PAD
            .decode(tag_b64)
            .map_err(|_| RequestStateError::InvalidEncoding)?;

        let mut mac = self.mac();
        mac.update(&payload);
        // `verify_slice` compares in constant time and rejects wrong-length tags.
        mac.verify_slice(&tag)
            .map_err(|_| RequestStateError::IntegrityCheckFailed)?;

        Ok(payload)
    }

    /// Opens a sealed value and deserializes its JSON payload.
    ///
    /// # Errors
    ///
    /// Returns the same integrity and format errors as [`Self::open`], plus
    /// [`RequestStateError::Deserialization`] if the payload is not valid JSON
    /// for `T`.
    pub fn open_json<T: DeserializeOwned>(&self, sealed: &str) -> Result<T, RequestStateError> {
        let payload = self.open(sealed)?;
        serde_json::from_slice(&payload).map_err(RequestStateError::Deserialization)
    }

    /// Builds an HMAC instance keyed for request-state tags, pre-fed with the
    /// domain-separation label so `seal` and `open` stay in agreement.
    fn mac(&self) -> HmacSha256 {
        let mut mac =
            HmacSha256::new_from_slice(&self.key).expect("HMAC accepts keys of any length");
        mac.update(DOMAIN);
        mac
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_roundtrips_bytes() {
        let codec = RequestStateCodec::new(b"test-key-test-key-test-key-32byte".to_vec());
        let sealed = codec.seal(b"hello world");
        assert!(sealed.starts_with("rs1."));
        assert_eq!(codec.open(&sealed).unwrap(), b"hello world");
    }

    #[test]
    fn seal_open_roundtrips_json() {
        #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
        struct State {
            tool: String,
            round: u32,
        }
        let codec = RequestStateCodec::new(b"another-strong-signing-key-here!!".to_vec());
        let state = State {
            tool: "weather".into(),
            round: 3,
        };
        let sealed = codec.seal_json(&state).unwrap();
        let opened: State = codec.open_json(&sealed).unwrap();
        assert_eq!(opened, state);
    }

    #[test]
    fn empty_payload_roundtrips() {
        let codec = RequestStateCodec::new(b"k".to_vec());
        let sealed = codec.seal(b"");
        assert_eq!(codec.open(&sealed).unwrap(), b"");
    }

    #[test]
    fn tampered_payload_is_rejected() {
        let codec = RequestStateCodec::new(b"signing-key-signing-key-signing!!".to_vec());
        let sealed = codec.seal(b"amount=100");

        // Flip the payload section but keep the original tag.
        let mut parts: Vec<&str> = sealed.split('.').collect();
        let forged_payload = URL_SAFE_NO_PAD.encode(b"amount=999");
        parts[1] = &forged_payload;
        let forged = parts.join(".");

        assert!(matches!(
            codec.open(&forged),
            Err(RequestStateError::IntegrityCheckFailed)
        ));
    }

    #[test]
    fn different_key_is_rejected() {
        let signer = RequestStateCodec::new(b"the-real-signing-key-value-here!!".to_vec());
        let attacker = RequestStateCodec::new(b"a-totally-different-forged-key!!!".to_vec());
        let sealed = signer.seal(b"trusted");
        assert!(matches!(
            attacker.open(&sealed),
            Err(RequestStateError::IntegrityCheckFailed)
        ));
    }

    #[test]
    fn appended_bytes_are_rejected() {
        let codec = RequestStateCodec::new(b"key-key-key-key-key-key-key-key!!".to_vec());
        let mut sealed = codec.seal(b"state");
        sealed.push('x');
        assert!(codec.open(&sealed).is_err());
    }

    #[test]
    fn wrong_version_prefix_is_malformed() {
        let codec = RequestStateCodec::new(b"key".to_vec());
        let sealed = codec.seal(b"state");
        let bumped = sealed.replacen("rs1.", "rs2.", 1);
        assert!(matches!(
            codec.open(&bumped),
            Err(RequestStateError::MalformedFormat)
        ));
    }

    #[test]
    fn missing_sections_are_malformed() {
        let codec = RequestStateCodec::new(b"key".to_vec());
        assert!(matches!(
            codec.open("rs1"),
            Err(RequestStateError::MalformedFormat)
        ));
        assert!(matches!(
            codec.open("rs1.onlypayload"),
            Err(RequestStateError::MalformedFormat)
        ));
        assert!(matches!(
            codec.open("rs1.a.b.c"),
            Err(RequestStateError::MalformedFormat)
        ));
    }

    #[test]
    fn non_base64_sections_are_invalid_encoding() {
        let codec = RequestStateCodec::new(b"key".to_vec());
        // '.' and '+'/'/' are not valid in URL-safe base64; use an invalid char.
        assert!(matches!(
            codec.open("rs1.!!!!.!!!!"),
            Err(RequestStateError::InvalidEncoding)
        ));
    }

    #[test]
    fn debug_does_not_leak_key() {
        let codec = RequestStateCodec::new(b"super-secret-key".to_vec());
        let rendered = format!("{codec:?}");
        assert!(!rendered.contains("super-secret-key"));
        assert!(rendered.contains("redacted"));
    }
}
