//! Integrity protection for SEP-2322 `requestState`.
//!
//! In the multi round-trip request (MRTR) flow, a server places an opaque
//! `requestState` string in an [`InputRequiredResult`](super::InputRequiredResult)
//! and the client echoes it back verbatim on retry. From the server's point of
//! view the echoed value is **untrusted, attacker-controlled input**: a client
//! can send back anything it likes. Per SEP-2322, a server that lets
//! `requestState` influence authorization, resource access, or business logic
//! MUST protect its integrity and reject values that fail verification.
//!
//! [`RequestStateCodec`] provides an opt-in way to do this. It seals a payload
//! into an opaque string with an HMAC-SHA256 tag and opens it again, rejecting
//! any value that was forged or tampered with.
//!
//! To follow the spec's replay-prevention guidance without hand-rolling the
//! checks, the codec supports two bindings via [`SealOptions`]:
//!
//! * **Associated data** — arbitrary context (e.g. the authenticated principal
//!   plus a digest of the originating request) that is mixed into the tag but
//!   not stored in the token. [`open_with`](RequestStateCodec::open_with) only
//!   succeeds when the caller supplies the same context, so a value cannot be
//!   replayed by a different principal or against a different request. This is
//!   *fail-closed*: forgetting to pass the context makes verification fail.
//! * **TTL** — a relative expiry stamped into the token; opening a value past
//!   its expiry fails with [`RequestStateError::Expired`].
//!
//! Single-use/nonce enforcement (for one-time redemptions) still has to be done
//! server-side, as the spec notes.
//!
//! This helper is only about *integrity*, not *confidentiality*: the sealed
//! payload is signed, not encrypted, so it is base64url-readable by anyone. Do
//! not put secrets in it.
//!
//! Using the codec is entirely optional. A server that keeps its state
//! server-side, or that does not trust `requestState` for anything security
//! sensitive, can keep building the string by hand via
//! [`InputRequiredResult::from_request_state`](super::InputRequiredResult::from_request_state).
//!
//! # Examples
//!
//! ```
//! use rmcp::model::{RequestStateCodec, SealOptions};
//!
//! // Derive the key from a per-process secret; keep it out of client reach.
//! let codec = RequestStateCodec::new(b"a-32-byte-or-longer-secret-key!!!");
//!
//! // Bind the state to the caller and the originating request.
//! let context = b"user:alice|tools/call:weather";
//! let sealed = codec.seal_with(
//!     b"step=2",
//!     &SealOptions::new().associated_data(context),
//! );
//!
//! // On retry the client echoes `sealed` back untouched; the server re-derives
//! // the same context and opens it.
//! let opened = codec.open_with(&sealed, context).expect("integrity check passes");
//! assert_eq!(opened, b"step=2");
//!
//! // A different principal (different context) is rejected.
//! assert!(codec.open_with(&sealed, b"user:bob|tools/call:weather").is_err());
//! ```

use std::time::Duration;

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

/// Length of the big-endian expiry prefix (unix milliseconds) stored at the
/// front of every sealed body. `0` means "no expiry".
const EXPIRY_LEN: usize = 8;

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

    /// The HMAC tag did not match; the value was forged, tampered with, or
    /// opened with the wrong associated data.
    #[error("request state failed integrity verification")]
    IntegrityCheckFailed,

    /// The value carried a TTL that has already elapsed.
    #[error("request state has expired")]
    Expired,

    /// The sealed payload could not be serialized to JSON.
    #[error("failed to serialize request state payload: {0}")]
    Serialization(#[source] serde_json::Error),

    /// The opened payload could not be deserialized from JSON.
    #[error("failed to deserialize request state payload: {0}")]
    Deserialization(#[source] serde_json::Error),
}

/// Options controlling how a value is sealed by [`RequestStateCodec`].
///
/// Defaults to no associated data and no expiry, which is equivalent to the
/// bare [`seal`](RequestStateCodec::seal) / [`open`](RequestStateCodec::open)
/// methods.
#[derive(Clone, Copy, Debug, Default)]
pub struct SealOptions<'a> {
    associated_data: &'a [u8],
    ttl: Option<Duration>,
}

impl<'a> SealOptions<'a> {
    /// Creates empty options (no associated data, no expiry).
    pub fn new() -> Self {
        Self::default()
    }

    /// Binds the sealed value to `associated_data`. The same bytes must be
    /// supplied to [`open_with`](RequestStateCodec::open_with); the data is
    /// authenticated but not stored in the token.
    ///
    /// Use this to bind the state to the authenticated principal and/or the
    /// originating request (e.g. method name plus a digest of its parameters).
    pub fn associated_data(mut self, associated_data: &'a [u8]) -> Self {
        self.associated_data = associated_data;
        self
    }

    /// Sets a relative time-to-live after which opening the value fails with
    /// [`RequestStateError::Expired`].
    pub fn ttl(mut self, ttl: Duration) -> Self {
        self.ttl = Some(ttl);
        self
    }
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
        self.seal_with(payload, &SealOptions::default())
    }

    /// Seals raw bytes with [`SealOptions`] (associated data and/or TTL).
    pub fn seal_with(&self, payload: &[u8], options: &SealOptions<'_>) -> String {
        self.seal_at(payload, options, Self::now_ms())
    }

    /// Seals a serializable value by encoding it as JSON before sealing.
    ///
    /// # Errors
    ///
    /// Returns [`RequestStateError::Serialization`] if `value` cannot be encoded
    /// as JSON.
    pub fn seal_json<T: Serialize>(&self, value: &T) -> Result<String, RequestStateError> {
        self.seal_json_with(value, &SealOptions::default())
    }

    /// Seals a serializable value with [`SealOptions`].
    ///
    /// # Errors
    ///
    /// Returns [`RequestStateError::Serialization`] if `value` cannot be encoded
    /// as JSON.
    pub fn seal_json_with<T: Serialize>(
        &self,
        value: &T,
        options: &SealOptions<'_>,
    ) -> Result<String, RequestStateError> {
        let payload = serde_json::to_vec(value).map_err(RequestStateError::Serialization)?;
        Ok(self.seal_with(&payload, options))
    }

    /// Opens a sealed value that was sealed without associated data, verifying
    /// its integrity and expiry and returning the original bytes.
    ///
    /// # Errors
    ///
    /// See [`open_with`](Self::open_with).
    pub fn open(&self, sealed: &str) -> Result<Vec<u8>, RequestStateError> {
        self.open_with(sealed, &[])
    }

    /// Opens a sealed value, verifying its integrity against `associated_data`
    /// and checking its expiry.
    ///
    /// `associated_data` must match the bytes passed to
    /// [`SealOptions::associated_data`] when the value was sealed (use `&[]` for
    /// values sealed without it).
    ///
    /// # Errors
    ///
    /// - [`RequestStateError::IntegrityCheckFailed`] if the value was not
    ///   produced by this key or the associated data differs.
    /// - [`RequestStateError::Expired`] if the value's TTL has elapsed.
    /// - [`RequestStateError::MalformedFormat`] or
    ///   [`RequestStateError::InvalidEncoding`] if it is not a well-formed sealed
    ///   value.
    pub fn open_with(
        &self,
        sealed: &str,
        associated_data: &[u8],
    ) -> Result<Vec<u8>, RequestStateError> {
        self.open_at(sealed, associated_data, Self::now_ms())
    }

    /// Opens a sealed value (no associated data) and deserializes its JSON
    /// payload.
    ///
    /// # Errors
    ///
    /// See [`open_json_with`](Self::open_json_with).
    pub fn open_json<T: DeserializeOwned>(&self, sealed: &str) -> Result<T, RequestStateError> {
        self.open_json_with(sealed, &[])
    }

    /// Opens a sealed value against `associated_data` and deserializes its JSON
    /// payload.
    ///
    /// # Errors
    ///
    /// Returns the same integrity, expiry, and format errors as
    /// [`open_with`](Self::open_with), plus [`RequestStateError::Deserialization`]
    /// if the payload is not valid JSON for `T`.
    pub fn open_json_with<T: DeserializeOwned>(
        &self,
        sealed: &str,
        associated_data: &[u8],
    ) -> Result<T, RequestStateError> {
        let payload = self.open_with(sealed, associated_data)?;
        serde_json::from_slice(&payload).map_err(RequestStateError::Deserialization)
    }

    fn seal_at(&self, payload: &[u8], options: &SealOptions<'_>, now_ms: i64) -> String {
        let expiry = match options.ttl {
            Some(ttl) => now_ms.saturating_add(ttl.as_millis().min(i64::MAX as u128) as i64),
            None => 0,
        };

        // body = big-endian expiry (0 = none) followed by the caller payload.
        let mut body = Vec::with_capacity(EXPIRY_LEN + payload.len());
        body.extend_from_slice(&expiry.to_be_bytes());
        body.extend_from_slice(payload);

        let tag = self
            .mac_for(options.associated_data, &body)
            .finalize()
            .into_bytes();

        // base64url without padding encodes 3 bytes as 4 chars, rounding up.
        let b64_len = |n: usize| n.div_ceil(3) * 4;
        let mut out =
            String::with_capacity(VERSION.len() + 2 + b64_len(body.len()) + b64_len(tag.len()));
        out.push_str(VERSION);
        out.push('.');
        URL_SAFE_NO_PAD.encode_string(&body, &mut out);
        out.push('.');
        URL_SAFE_NO_PAD.encode_string(tag.as_slice(), &mut out);
        out
    }

    fn open_at(
        &self,
        sealed: &str,
        associated_data: &[u8],
        now_ms: i64,
    ) -> Result<Vec<u8>, RequestStateError> {
        let mut parts = sealed.split('.');
        let version = parts.next().ok_or(RequestStateError::MalformedFormat)?;
        let body_b64 = parts.next().ok_or(RequestStateError::MalformedFormat)?;
        let tag_b64 = parts.next().ok_or(RequestStateError::MalformedFormat)?;
        if parts.next().is_some() || version != VERSION {
            return Err(RequestStateError::MalformedFormat);
        }

        let body = URL_SAFE_NO_PAD
            .decode(body_b64)
            .map_err(|_| RequestStateError::InvalidEncoding)?;
        let tag = URL_SAFE_NO_PAD
            .decode(tag_b64)
            .map_err(|_| RequestStateError::InvalidEncoding)?;

        // `verify_slice` compares in constant time and rejects wrong-length tags.
        self.mac_for(associated_data, &body)
            .verify_slice(&tag)
            .map_err(|_| RequestStateError::IntegrityCheckFailed)?;

        // The body is now authenticated, so its framing can be trusted.
        if body.len() < EXPIRY_LEN {
            return Err(RequestStateError::MalformedFormat);
        }
        let expiry = i64::from_be_bytes(body[..EXPIRY_LEN].try_into().expect("checked length"));
        if expiry != 0 && now_ms > expiry {
            return Err(RequestStateError::Expired);
        }

        Ok(body[EXPIRY_LEN..].to_vec())
    }

    /// Builds an HMAC keyed for request-state tags, pre-fed with the
    /// domain-separation label, a length-prefixed `associated_data`, and the
    /// body. The length prefix keeps the `associated_data`/`body` boundary
    /// unambiguous so distinct inputs cannot collide.
    fn mac_for(&self, associated_data: &[u8], body: &[u8]) -> HmacSha256 {
        let mut mac =
            HmacSha256::new_from_slice(&self.key).expect("HMAC accepts keys of any length");
        mac.update(DOMAIN);
        mac.update(&(associated_data.len() as u64).to_be_bytes());
        mac.update(associated_data);
        mac.update(body);
        mac
    }

    fn now_ms() -> i64 {
        chrono::Utc::now().timestamp_millis()
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

        // Replace the body section but keep the original tag.
        let mut parts: Vec<&str> = sealed.split('.').collect();
        let forged_body = URL_SAFE_NO_PAD.encode(b"amount=999");
        parts[1] = &forged_body;
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
            codec.open("rs1.onlybody"),
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

    mod associated_data {
        use super::*;

        #[test]
        fn matching_context_opens() {
            let codec = RequestStateCodec::new(b"key-key-key-key-key-key-key-key!!".to_vec());
            let ctx = b"user:alice|tools/call:weather";
            let sealed = codec.seal_with(b"state", &SealOptions::new().associated_data(ctx));
            assert_eq!(codec.open_with(&sealed, ctx).unwrap(), b"state");
        }

        #[test]
        fn different_context_is_rejected() {
            let codec = RequestStateCodec::new(b"key-key-key-key-key-key-key-key!!".to_vec());
            let sealed =
                codec.seal_with(b"state", &SealOptions::new().associated_data(b"user:alice"));
            assert!(matches!(
                codec.open_with(&sealed, b"user:bob"),
                Err(RequestStateError::IntegrityCheckFailed)
            ));
        }

        #[test]
        fn missing_context_is_rejected() {
            let codec = RequestStateCodec::new(b"key-key-key-key-key-key-key-key!!".to_vec());
            let sealed =
                codec.seal_with(b"state", &SealOptions::new().associated_data(b"user:alice"));
            // Opening without the associated data must fail closed.
            assert!(matches!(
                codec.open(&sealed),
                Err(RequestStateError::IntegrityCheckFailed)
            ));
        }
    }

    mod ttl {
        use super::*;

        const KEY: &[u8] = b"ttl-signing-key-ttl-signing-key!!";

        #[test]
        fn within_ttl_opens() {
            let codec = RequestStateCodec::new(KEY.to_vec());
            let sealed = codec.seal_at(
                b"state",
                &SealOptions::new().ttl(Duration::from_secs(60)),
                1_000,
            );
            // 30s later, still valid.
            assert_eq!(codec.open_at(&sealed, &[], 31_000).unwrap(), b"state");
        }

        #[test]
        fn past_ttl_is_expired() {
            let codec = RequestStateCodec::new(KEY.to_vec());
            let sealed = codec.seal_at(
                b"state",
                &SealOptions::new().ttl(Duration::from_secs(60)),
                1_000,
            );
            // 61s later, expired.
            assert!(matches!(
                codec.open_at(&sealed, &[], 62_000),
                Err(RequestStateError::Expired)
            ));
        }

        #[test]
        fn no_ttl_never_expires() {
            let codec = RequestStateCodec::new(KEY.to_vec());
            let sealed = codec.seal_at(b"state", &SealOptions::new(), 1_000);
            assert_eq!(codec.open_at(&sealed, &[], i64::MAX).unwrap(), b"state");
        }

        #[test]
        fn ttl_and_associated_data_combine() {
            let codec = RequestStateCodec::new(KEY.to_vec());
            let ctx = b"user:alice";
            let sealed = codec.seal_at(
                b"state",
                &SealOptions::new()
                    .associated_data(ctx)
                    .ttl(Duration::from_secs(60)),
                1_000,
            );
            assert_eq!(codec.open_at(&sealed, ctx, 10_000).unwrap(), b"state");
            assert!(matches!(
                codec.open_at(&sealed, b"user:bob", 10_000),
                Err(RequestStateError::IntegrityCheckFailed)
            ));
            assert!(matches!(
                codec.open_at(&sealed, ctx, 99_000),
                Err(RequestStateError::Expired)
            ));
        }
    }
}
