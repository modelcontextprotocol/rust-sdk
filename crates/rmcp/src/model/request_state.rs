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

use std::{
    collections::{HashMap, hash_map::Entry},
    time::Duration,
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, KeyInit, Mac};
use serde::{Serialize, de::DeserializeOwned};
use sha2::Sha256;
use thiserror::Error;

type HmacSha256 = Hmac<Sha256>;

/// Legacy version tag used by single-key and transitional codecs.
const VERSION_V1: &str = "rs1";

/// Keyed version tag used by keyring codecs after promotion.
const VERSION_V2: &str = "rs2";

/// Domain-separation label mixed into the HMAC so a `requestState` tag can never
/// be confused with an HMAC computed for some other purpose using the same key.
const DOMAIN_V1: &[u8] = b"rmcp/mrtr/request-state/v1";

/// Domain-separation label for keyed request-state tags.
const DOMAIN_V2: &[u8] = b"rmcp/mrtr/request-state/v2";

/// Length of the big-endian expiry prefix (unix milliseconds) stored at the
/// front of every sealed body. `0` means "no expiry".
const EXPIRY_LEN: usize = 8;

/// Maximum decoded UTF-8 byte length of an rs2 key id.
const MAX_KID_LEN: usize = 255;

/// Maximum unpadded base64url length for [`MAX_KID_LEN`] bytes.
const MAX_ENCODED_KID_LEN: usize = 340;

/// Errors returned when configuring, sealing, or opening a
/// [`RequestStateCodec`] value.
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

    /// The token names (or, for rs1, requires) a key this codec does not hold.
    #[error("request state was sealed with an unknown key")]
    UnknownKeyId,

    /// The decoded rs2 key id was empty, too long, or not valid UTF-8.
    #[error("request state contains an invalid key identifier")]
    InvalidKeyId,

    /// A keyring constructor or builder was given an invalid configuration.
    /// The message is for diagnostics and should not be matched programmatically.
    #[error("invalid keyring configuration: {0}")]
    InvalidKeyring(&'static str),

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
/// [`new`](Self::new) preserves `rs1`;
/// [`new_with_keyring`](Self::new_with_keyring) emits `rs2`.
/// Use [`with_rs1_signing`](Self::with_rs1_signing) and
/// [`with_rs1_fallback`](Self::with_rs1_fallback) for rolling migrations.
///
/// Use high-entropy keys of at least 32 bytes and configure the same keyring on
/// every replica that may continue an MRTR exchange.
/// This codec provides integrity, not confidentiality: both the `rs2` key id
/// and payload are base64url-readable by clients and are not encrypted.
///
/// # Wire format
///
/// Keyring codecs emit
/// `rs2.<base64url(kid)>.<base64url(expiry || payload)>.<base64url(tag)>`.
/// The expiry is a signed big-endian Unix timestamp in milliseconds, or zero
/// for no expiry. The tag authenticates the key id, associated data, and body
/// under an `rs2`-specific domain. Key ids are visible, authenticated,
/// case-sensitive UTF-8 strings; they are not confidential.
///
/// # Key rotation
///
/// Rotate in stages so every serving replica can open tokens emitted by peers:
///
/// 1. Deploy both keys everywhere while continuing to sign `rs1` with the old key.
/// 2. Activate the new key for `rs2`, retaining the old key as an `rs1` fallback.
/// 3. Wait out the maximum request-state lifetime, then remove the old key.
///
/// ```
/// # use rmcp::model::{RequestStateCodec, RequestStateError};
/// # fn configure() -> Result<(), RequestStateError> {
/// # let old = b"old-request-state-key-at-least-32b".as_slice();
/// # let new = b"new-request-state-key-at-least-32b".as_slice();
/// # let keys = [("old", old), ("new", new)];
/// let transitional =
///     RequestStateCodec::new_with_keyring("new", keys)?.with_rs1_signing("old")?;
/// let promoted =
///     RequestStateCodec::new_with_keyring("new", keys)?.with_rs1_fallback("old")?;
/// # Ok(())
/// # }
/// # configure().unwrap();
/// ```
///
/// For later `rs2` rotations, deploy both keys with the old key active, promote
/// the new key, wait for old states to drain, and then remove the old key.
/// The codec has no default TTL or runtime key reload. Without a bounded state
/// lifetime, an old verification key cannot be retired safely.
#[derive(Clone)]
pub struct RequestStateCodec {
    keys: Keys,
}

#[derive(Clone)]
enum Keys {
    Single(Box<[u8]>),
    Ring {
        keys: HashMap<String, Box<[u8]>>,
        seal_mode: SealMode,
        rs1_fallbacks: Vec<String>,
    },
}

#[derive(Clone, Debug)]
enum SealMode {
    Rs1 { key_id: String },
    Rs2 { key_id: String },
}

impl std::fmt::Debug for RequestStateCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never leak signing or verification keys through Debug output.
        match &self.keys {
            Keys::Single(_) => f
                .debug_struct("RequestStateCodec")
                .field("mode", &"single")
                .field("key", &"<redacted>")
                .finish(),
            Keys::Ring {
                keys,
                seal_mode,
                rs1_fallbacks,
            } => f
                .debug_struct("RequestStateCodec")
                .field("mode", &"ring")
                .field("key_count", &keys.len())
                .field("keys", &"<redacted>")
                .field("seal_mode", seal_mode)
                .field("rs1_fallbacks", rs1_fallbacks)
                .finish(),
        }
    }
}

impl RequestStateCodec {
    /// Creates a legacy single-key codec that seals and opens `rs1` values.
    pub fn new(key: impl Into<Vec<u8>>) -> Self {
        Self {
            keys: Keys::Single(key.into().into_boxed_slice()),
        }
    }

    /// Creates a keyring codec that seals `rs2` with `active_kid`.
    ///
    /// Opening an `rs2` value selects the named key from all configured keys;
    /// successful tag verification authenticates that key id. `active_kid`
    /// affects sealing only.
    ///
    /// Keyring codecs do not open legacy `rs1` values unless a fallback is
    /// added with [`with_rs1_fallback`](Self::with_rs1_fallback), or
    /// transitional signing is enabled with
    /// [`with_rs1_signing`](Self::with_rs1_signing).
    ///
    /// Key ids are opaque, case-sensitive UTF-8 strings of 1 to 255 bytes.
    ///
    /// # Errors
    ///
    /// Returns [`RequestStateError::InvalidKeyring`] if the keyring is empty,
    /// contains duplicate or invalid key ids, or does not contain `active_kid`.
    pub fn new_with_keyring<K, V>(
        active_kid: impl Into<String>,
        keys: impl IntoIterator<Item = (K, V)>,
    ) -> Result<Self, RequestStateError>
    where
        K: Into<String>,
        V: Into<Vec<u8>>,
    {
        let mut ring = HashMap::new();
        for (kid, key) in keys {
            let kid = kid.into();
            Self::validate_config_kid(&kid)?;
            match ring.entry(kid) {
                Entry::Vacant(entry) => {
                    entry.insert(key.into().into_boxed_slice());
                }
                Entry::Occupied(_) => {
                    return Err(RequestStateError::InvalidKeyring("duplicate key id"));
                }
            }
        }

        if ring.is_empty() {
            return Err(RequestStateError::InvalidKeyring(
                "keyring must contain at least one key",
            ));
        }

        let active_kid = active_kid.into();
        Self::validate_config_kid(&active_kid)?;
        if !ring.contains_key(&active_kid) {
            return Err(RequestStateError::InvalidKeyring(
                "active key id is not present in the keyring",
            ));
        }

        Ok(Self {
            keys: Keys::Ring {
                keys: ring,
                seal_mode: SealMode::Rs2 { key_id: active_kid },
                rs1_fallbacks: Vec::new(),
            },
        })
    }

    /// Switches a keyring codec to transitional `rs1` signing with `kid`.
    ///
    /// The selected key is also added to the legacy `rs1` fallback set, so the
    /// codec can open its own output while continuing to accept `rs2` values.
    ///
    /// # Errors
    ///
    /// Returns [`RequestStateError::InvalidKeyring`] when called on a
    /// single-key codec created by [`new`](Self::new), or if the keyring does
    /// not contain `kid`.
    pub fn with_rs1_signing(mut self, kid: impl AsRef<str>) -> Result<Self, RequestStateError> {
        let kid = kid.as_ref();
        match &mut self.keys {
            Keys::Single(_) => Err(RequestStateError::InvalidKeyring(
                "rs1 transitional signing requires a keyring",
            )),
            Keys::Ring {
                keys,
                seal_mode,
                rs1_fallbacks,
            } => {
                if !keys.contains_key(kid) {
                    return Err(RequestStateError::InvalidKeyring(
                        "rs1 signing key id is not present in the keyring",
                    ));
                }

                let kid = kid.to_owned();
                *seal_mode = SealMode::Rs1 {
                    key_id: kid.clone(),
                };
                if !rs1_fallbacks.iter().any(|existing| existing == &kid) {
                    rs1_fallbacks.push(kid);
                }
                Ok(self)
            }
        }
    }

    /// Adds `kid` as an accepted key for legacy `rs1` values.
    ///
    /// Adding the same id more than once has no effect.
    ///
    /// Opening `rs1` evaluates every configured fallback before returning, so
    /// keep the fallback set small and remove it after old values have drained.
    ///
    /// # Errors
    ///
    /// Returns [`RequestStateError::InvalidKeyring`] when called on a
    /// single-key codec created by [`new`](Self::new), or if the keyring does
    /// not contain `kid`.
    pub fn with_rs1_fallback(mut self, kid: impl AsRef<str>) -> Result<Self, RequestStateError> {
        let kid = kid.as_ref();
        match &mut self.keys {
            Keys::Single(_) => Err(RequestStateError::InvalidKeyring(
                "rs1 fallbacks require a keyring",
            )),
            Keys::Ring {
                keys,
                rs1_fallbacks,
                ..
            } => {
                if !keys.contains_key(kid) {
                    return Err(RequestStateError::InvalidKeyring(
                        "rs1 fallback key id is not present in the keyring",
                    ));
                }
                if !rs1_fallbacks.iter().any(|existing| existing == kid) {
                    rs1_fallbacks.push(kid.to_owned());
                }
                Ok(self)
            }
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
    /// - [`RequestStateError::UnknownKeyId`] if the codec cannot select a key
    ///   for a structurally valid value.
    /// - [`RequestStateError::IntegrityCheckFailed`] if the tag does not match
    ///   the selected key or the associated data differs.
    /// - [`RequestStateError::Expired`] if the value's TTL has elapsed.
    /// - [`RequestStateError::MalformedFormat`] or
    ///   [`RequestStateError::InvalidEncoding`] if it is not a well-formed sealed
    ///   value.
    /// - [`RequestStateError::InvalidKeyId`] if an `rs2` key id is empty, too
    ///   long, or not valid UTF-8.
    ///
    /// Applications MUST map all token-opening failures to a single
    /// client-facing error and reserve the detailed variants for internal
    /// diagnostics.
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

        match &self.keys {
            Keys::Single(key) => Self::seal_rs1(key, options.associated_data, &body),
            Keys::Ring {
                keys, seal_mode, ..
            } => match seal_mode {
                SealMode::Rs1 { key_id } => Self::seal_rs1(
                    keys.get(key_id).expect("validated rs1 signing key"),
                    options.associated_data,
                    &body,
                ),
                SealMode::Rs2 { key_id } => Self::seal_rs2(
                    key_id,
                    keys.get(key_id).expect("validated rs2 signing key"),
                    options.associated_data,
                    &body,
                ),
            },
        }
    }

    fn open_at(
        &self,
        sealed: &str,
        associated_data: &[u8],
        now_ms: i64,
    ) -> Result<Vec<u8>, RequestStateError> {
        match sealed.split('.').next() {
            Some(VERSION_V1) => self.open_rs1_at(sealed, associated_data, now_ms),
            Some(VERSION_V2) => self.open_rs2_at(sealed, associated_data, now_ms),
            _ => Err(RequestStateError::MalformedFormat),
        }
    }

    fn seal_rs1(key: &[u8], associated_data: &[u8], body: &[u8]) -> String {
        let tag = Self::mac_v1(key, associated_data, body)
            .finalize()
            .into_bytes();
        let mut out = String::with_capacity(
            VERSION_V1.len() + 2 + Self::b64_len(body.len()) + Self::b64_len(tag.len()),
        );
        out.push_str(VERSION_V1);
        out.push('.');
        URL_SAFE_NO_PAD.encode_string(body, &mut out);
        out.push('.');
        URL_SAFE_NO_PAD.encode_string(tag.as_slice(), &mut out);
        out
    }

    fn seal_rs2(kid: &str, key: &[u8], associated_data: &[u8], body: &[u8]) -> String {
        let tag = Self::mac_v2(key, kid.as_bytes(), associated_data, body)
            .finalize()
            .into_bytes();
        let mut out = String::with_capacity(
            VERSION_V2.len()
                + 3
                + Self::b64_len(kid.len())
                + Self::b64_len(body.len())
                + Self::b64_len(tag.len()),
        );
        out.push_str(VERSION_V2);
        out.push('.');
        URL_SAFE_NO_PAD.encode_string(kid.as_bytes(), &mut out);
        out.push('.');
        URL_SAFE_NO_PAD.encode_string(body, &mut out);
        out.push('.');
        URL_SAFE_NO_PAD.encode_string(tag.as_slice(), &mut out);
        out
    }

    fn open_rs1_at(
        &self,
        sealed: &str,
        associated_data: &[u8],
        now_ms: i64,
    ) -> Result<Vec<u8>, RequestStateError> {
        let mut parts = sealed.split('.');
        let version = parts.next().ok_or(RequestStateError::MalformedFormat)?;
        let body_b64 = parts.next().ok_or(RequestStateError::MalformedFormat)?;
        let tag_b64 = parts.next().ok_or(RequestStateError::MalformedFormat)?;
        if parts.next().is_some() || version != VERSION_V1 {
            return Err(RequestStateError::MalformedFormat);
        }

        if matches!(
            &self.keys,
            Keys::Ring { rs1_fallbacks, .. } if rs1_fallbacks.is_empty()
        ) {
            return Err(RequestStateError::UnknownKeyId);
        }

        let body = URL_SAFE_NO_PAD
            .decode(body_b64)
            .map_err(|_| RequestStateError::InvalidEncoding)?;
        let tag = URL_SAFE_NO_PAD
            .decode(tag_b64)
            .map_err(|_| RequestStateError::InvalidEncoding)?;

        // `verify_slice` compares tags in constant time and rejects wrong-length tags.
        match &self.keys {
            Keys::Single(key) => Self::mac_v1(key, associated_data, &body)
                .verify_slice(&tag)
                .map_err(|_| RequestStateError::IntegrityCheckFailed)?,
            Keys::Ring {
                keys,
                rs1_fallbacks,
                ..
            } => {
                // Evaluate every fallback so the HMAC count does not reveal which key matched.
                let mut verified = false;
                for kid in rs1_fallbacks {
                    let key = keys.get(kid).expect("validated rs1 fallback key");
                    let matches = Self::mac_v1(key, associated_data, &body)
                        .verify_slice(&tag)
                        .is_ok();
                    verified |= matches;
                }
                if !verified {
                    return Err(RequestStateError::IntegrityCheckFailed);
                }
            }
        }

        Self::open_authenticated_body(body, now_ms)
    }

    fn open_rs2_at(
        &self,
        sealed: &str,
        associated_data: &[u8],
        now_ms: i64,
    ) -> Result<Vec<u8>, RequestStateError> {
        let mut parts = sealed.split('.');
        let version = parts.next().ok_or(RequestStateError::MalformedFormat)?;
        let kid_b64 = parts.next().ok_or(RequestStateError::MalformedFormat)?;
        let body_b64 = parts.next().ok_or(RequestStateError::MalformedFormat)?;
        let tag_b64 = parts.next().ok_or(RequestStateError::MalformedFormat)?;
        if parts.next().is_some() || version != VERSION_V2 {
            return Err(RequestStateError::MalformedFormat);
        }

        if kid_b64.len() > MAX_ENCODED_KID_LEN {
            return Err(RequestStateError::InvalidKeyId);
        }
        let kid = URL_SAFE_NO_PAD
            .decode(kid_b64)
            .map_err(|_| RequestStateError::InvalidEncoding)?;
        if kid.is_empty() || kid.len() > MAX_KID_LEN {
            return Err(RequestStateError::InvalidKeyId);
        }
        let kid = String::from_utf8(kid).map_err(|_| RequestStateError::InvalidKeyId)?;

        let key = match &self.keys {
            Keys::Single(_) => return Err(RequestStateError::UnknownKeyId),
            Keys::Ring { keys, .. } => keys.get(&kid).ok_or(RequestStateError::UnknownKeyId)?,
        };

        let body = URL_SAFE_NO_PAD
            .decode(body_b64)
            .map_err(|_| RequestStateError::InvalidEncoding)?;
        let tag = URL_SAFE_NO_PAD
            .decode(tag_b64)
            .map_err(|_| RequestStateError::InvalidEncoding)?;

        // `verify_slice` compares tags in constant time and rejects wrong-length tags.
        Self::mac_v2(key, kid.as_bytes(), associated_data, &body)
            .verify_slice(&tag)
            .map_err(|_| RequestStateError::IntegrityCheckFailed)?;

        Self::open_authenticated_body(body, now_ms)
    }

    fn open_authenticated_body(body: Vec<u8>, now_ms: i64) -> Result<Vec<u8>, RequestStateError> {
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

    fn mac_v1(key: &[u8], associated_data: &[u8], body: &[u8]) -> HmacSha256 {
        let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts keys of any length");
        mac.update(DOMAIN_V1);
        mac.update(&(associated_data.len() as u64).to_be_bytes());
        mac.update(associated_data);
        mac.update(body);
        mac
    }

    fn mac_v2(key: &[u8], kid: &[u8], associated_data: &[u8], body: &[u8]) -> HmacSha256 {
        let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts keys of any length");
        mac.update(DOMAIN_V2);
        mac.update(&(kid.len() as u64).to_be_bytes());
        mac.update(kid);
        mac.update(&(associated_data.len() as u64).to_be_bytes());
        mac.update(associated_data);
        mac.update(body);
        mac
    }

    fn validate_config_kid(kid: &str) -> Result<(), RequestStateError> {
        if kid.is_empty() {
            return Err(RequestStateError::InvalidKeyring(
                "key id must not be empty",
            ));
        }
        if kid.len() > MAX_KID_LEN {
            return Err(RequestStateError::InvalidKeyring(
                "key id exceeds 255 UTF-8 bytes",
            ));
        }
        Ok(())
    }

    // Base64url without padding encodes at most three bytes as four characters.
    fn b64_len(len: usize) -> usize {
        len.div_ceil(3) * 4
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
    fn unsupported_version_is_malformed() {
        let codec = RequestStateCodec::new(b"key".to_vec());
        let sealed = codec.seal(b"state");
        let bumped = sealed.replacen("rs1.", "rs3.", 1);
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

    mod key_rotation {
        use super::*;

        const KEY_A: &[u8] = b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        const KEY_B: &[u8] = b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        const KEY_C: &[u8] = b"cccccccccccccccccccccccccccccccc";
        const KAT_KEY: &[u8] = b"0123456789abcdef0123456789abcdef";
        const KAT_KID: &str = "rotation-key-2026-08";
        const KAT_AD: &[u8] = b"user:alice|request:weather";
        const KAT_NOW_MS: i64 = 1_700_000_000_000;

        fn two_key_ring(active: &str) -> RequestStateCodec {
            RequestStateCodec::new_with_keyring(active, [("a", KEY_A), ("b", KEY_B)]).unwrap()
        }

        fn body(expiry: i64, payload: &[u8]) -> Vec<u8> {
            let mut body = expiry.to_be_bytes().to_vec();
            body.extend_from_slice(payload);
            body
        }

        fn replace_segment(token: &str, index: usize, replacement: &str) -> String {
            let mut parts: Vec<String> = token.split('.').map(str::to_owned).collect();
            parts[index] = replacement.to_owned();
            parts.join(".")
        }

        fn test_mac(
            key: &[u8],
            domain: &[u8],
            kid: Option<&[u8]>,
            associated_data: &[u8],
            body: &[u8],
        ) -> Vec<u8> {
            let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts keys of any length");
            mac.update(domain);
            if let Some(kid) = kid {
                mac.update(&(kid.len() as u64).to_be_bytes());
                mac.update(kid);
            }
            mac.update(&(associated_data.len() as u64).to_be_bytes());
            mac.update(associated_data);
            mac.update(body);
            mac.finalize().into_bytes().to_vec()
        }

        fn raw_rs1(body: &[u8], tag: &[u8]) -> String {
            format!(
                "rs1.{}.{}",
                URL_SAFE_NO_PAD.encode(body),
                URL_SAFE_NO_PAD.encode(tag)
            )
        }

        fn raw_rs2(kid: &[u8], body: &[u8], tag: &[u8]) -> String {
            format!(
                "rs2.{}.{}.{}",
                URL_SAFE_NO_PAD.encode(kid),
                URL_SAFE_NO_PAD.encode(body),
                URL_SAFE_NO_PAD.encode(tag)
            )
        }

        #[test]
        fn rs1_known_answer_is_unchanged() {
            // Independently checked with Python stdlib HMAC and Ruby OpenSSL.
            let codec = RequestStateCodec::new(KAT_KEY);
            let sealed = codec.seal_at(
                b"step=2",
                &SealOptions::new()
                    .associated_data(KAT_AD)
                    .ttl(Duration::from_secs(90)),
                KAT_NOW_MS,
            );
            assert_eq!(
                sealed,
                "rs1.AAABi8_mx5BzdGVwPTI.GQgS0X7mtSz8ZOy_kld2Zjuc4gAMGpBL74EghWI36IQ"
            );
        }

        #[test]
        fn rs2_known_answer_matches_wire_specification() {
            // Independently checked with Python stdlib HMAC and Ruby OpenSSL.
            let codec = RequestStateCodec::new_with_keyring(KAT_KID, [(KAT_KID, KAT_KEY)]).unwrap();
            let sealed = codec.seal_at(
                b"step=2",
                &SealOptions::new()
                    .associated_data(KAT_AD)
                    .ttl(Duration::from_secs(90)),
                KAT_NOW_MS,
            );
            assert_eq!(
                sealed,
                "rs2.cm90YXRpb24ta2V5LTIwMjYtMDg.AAABi8_mx5BzdGVwPTI.twv2acu7lKXqebyrmit-JrHHZm-BkKlBQEMMtL3lHk8"
            );
        }

        #[test]
        fn rs2_roundtrips_bytes_json_associated_data_and_ttl() {
            let codec = two_key_ring("a");
            let options = SealOptions::new()
                .associated_data(b"user:alice")
                .ttl(Duration::from_secs(60));

            let sealed = codec.seal_at(b"", &options, 1_000);
            assert!(sealed.starts_with("rs2.YQ."));
            assert_eq!(codec.open_at(&sealed, b"user:alice", 30_000).unwrap(), b"");
            assert!(matches!(
                codec.open_at(&sealed, b"user:bob", 30_000),
                Err(RequestStateError::IntegrityCheckFailed)
            ));
            assert!(matches!(
                codec.open_at(&sealed, b"user:alice", 70_000),
                Err(RequestStateError::Expired)
            ));

            let value = serde_json::json!({ "step": 2, "tool": "weather" });
            let sealed = codec.seal_json(&value).unwrap();
            let opened: serde_json::Value = codec.open_json(&sealed).unwrap();
            assert_eq!(opened, value);
        }

        #[test]
        fn rotation_keeps_previous_rs2_key_verifiable() {
            let signer_a = two_key_ring("a");
            let token_a = signer_a.seal(b"state-a");

            let signer_b = two_key_ring("b");
            assert_eq!(signer_b.open(&token_a).unwrap(), b"state-a");
            let token_b = signer_b.seal(b"state-b");
            assert!(token_b.starts_with("rs2.Yg."));
            assert_eq!(signer_b.open(&token_b).unwrap(), b"state-b");

            let without_a = RequestStateCodec::new_with_keyring("b", [("b", KEY_B)]).unwrap();
            assert!(matches!(
                without_a.open(&token_a),
                Err(RequestStateError::UnknownKeyId)
            ));
        }

        #[test]
        fn rolling_migration_is_bidirectionally_compatible() {
            let old = RequestStateCodec::new(KEY_A);
            let transitional =
                RequestStateCodec::new_with_keyring("new", [("old", KEY_A), ("new", KEY_B)])
                    .unwrap()
                    .with_rs1_signing("old")
                    .unwrap();
            let promoted =
                RequestStateCodec::new_with_keyring("new", [("old", KEY_A), ("new", KEY_B)])
                    .unwrap()
                    .with_rs1_fallback("old")
                    .unwrap();
            let retired = RequestStateCodec::new_with_keyring("new", [("new", KEY_B)]).unwrap();

            let old_token = old.seal(b"old");
            assert_eq!(transitional.open(&old_token).unwrap(), b"old");

            let transitional_token = transitional.seal(b"transition");
            assert!(transitional_token.starts_with("rs1."));
            assert_eq!(old.open(&transitional_token).unwrap(), b"transition");
            assert_eq!(promoted.open(&transitional_token).unwrap(), b"transition");

            let promoted_token = promoted.seal(b"promoted");
            assert!(promoted_token.starts_with("rs2."));
            assert_eq!(transitional.open(&promoted_token).unwrap(), b"promoted");
            assert_eq!(retired.open(&promoted_token).unwrap(), b"promoted");
            assert!(matches!(
                retired.open(&old_token),
                Err(RequestStateError::UnknownKeyId)
            ));
        }

        #[test]
        fn multiple_legacy_fallbacks_are_supported() {
            let legacy_a = RequestStateCodec::new(KEY_A).seal(b"a");
            let legacy_b = RequestStateCodec::new(KEY_B).seal(b"b");
            let legacy_c = RequestStateCodec::new(KEY_C).seal(b"c");
            let ring = RequestStateCodec::new_with_keyring(
                "new",
                [("a", KEY_A), ("b", KEY_B), ("new", KEY_C)],
            )
            .unwrap()
            .with_rs1_fallback("a")
            .unwrap()
            .with_rs1_fallback("b")
            .unwrap();

            assert_eq!(ring.open(&legacy_a).unwrap(), b"a");
            assert_eq!(ring.open(&legacy_b).unwrap(), b"b");
            assert!(matches!(
                ring.open(&legacy_c),
                Err(RequestStateError::IntegrityCheckFailed)
            ));
        }

        #[test]
        fn kid_is_authenticated_even_when_ids_share_key_bytes() {
            let codec =
                RequestStateCodec::new_with_keyring("a", [("a", KEY_A), ("b", KEY_A)]).unwrap();
            let sealed = codec.seal(b"state");
            let swapped = replace_segment(&sealed, 1, &URL_SAFE_NO_PAD.encode(b"b"));
            assert!(matches!(
                codec.open(&swapped),
                Err(RequestStateError::IntegrityCheckFailed)
            ));
        }

        #[test]
        fn independent_rs2_segment_tampering_is_rejected() {
            let codec = two_key_ring("a");
            let sealed = codec.seal(b"state");

            let swapped_kid = replace_segment(&sealed, 1, &URL_SAFE_NO_PAD.encode(b"b"));
            let tampered_body = replace_segment(&sealed, 2, &URL_SAFE_NO_PAD.encode(b"changed"));
            let tampered_tag = replace_segment(&sealed, 3, &URL_SAFE_NO_PAD.encode([0_u8; 32]));

            for tampered in [swapped_kid, tampered_body, tampered_tag] {
                assert!(matches!(
                    codec.open(&tampered),
                    Err(RequestStateError::IntegrityCheckFailed)
                ));
            }
        }

        #[test]
        fn version_domains_are_cryptographically_separate() {
            let token_body = body(0, b"state");

            let wrong_v2_tag = test_mac(KEY_A, DOMAIN_V1, Some(b"a"), b"", &token_body);
            let wrong_v2 = raw_rs2(b"a", &token_body, &wrong_v2_tag);
            assert!(matches!(
                two_key_ring("a").open(&wrong_v2),
                Err(RequestStateError::IntegrityCheckFailed)
            ));

            let wrong_v1_tag = test_mac(KEY_A, DOMAIN_V2, None, b"", &token_body);
            let wrong_v1 = raw_rs1(&token_body, &wrong_v1_tag);
            assert!(matches!(
                RequestStateCodec::new(KEY_A).open(&wrong_v1),
                Err(RequestStateError::IntegrityCheckFailed)
            ));
        }

        #[test]
        fn constructor_invariants_are_enforced() {
            let empty =
                RequestStateCodec::new_with_keyring("a", std::iter::empty::<(&str, &[u8])>());
            assert!(matches!(empty, Err(RequestStateError::InvalidKeyring(_))));

            let duplicate = RequestStateCodec::new_with_keyring("a", [("a", KEY_A), ("a", KEY_B)]);
            assert!(matches!(
                duplicate,
                Err(RequestStateError::InvalidKeyring(_))
            ));

            let empty_kid = RequestStateCodec::new_with_keyring("", [("", KEY_A)]);
            assert!(matches!(
                empty_kid,
                Err(RequestStateError::InvalidKeyring(_))
            ));

            let oversized = "x".repeat(MAX_KID_LEN + 1);
            let oversized_kid =
                RequestStateCodec::new_with_keyring(oversized.clone(), [(oversized, KEY_A)]);
            assert!(matches!(
                oversized_kid,
                Err(RequestStateError::InvalidKeyring(_))
            ));

            let absent_active = RequestStateCodec::new_with_keyring("missing", [("a", KEY_A)]);
            assert!(matches!(
                absent_active,
                Err(RequestStateError::InvalidKeyring(_))
            ));

            assert!(matches!(
                RequestStateCodec::new(KEY_A).with_rs1_signing("a"),
                Err(RequestStateError::InvalidKeyring(_))
            ));
            assert!(matches!(
                two_key_ring("a").with_rs1_signing("missing"),
                Err(RequestStateError::InvalidKeyring(_))
            ));
            assert!(matches!(
                RequestStateCodec::new(KEY_A).with_rs1_fallback("a"),
                Err(RequestStateError::InvalidKeyring(_))
            ));
            assert!(matches!(
                two_key_ring("a").with_rs1_fallback("missing"),
                Err(RequestStateError::InvalidKeyring(_))
            ));
        }

        #[test]
        fn maximum_length_kid_roundtrips_and_oversized_wire_kid_is_rejected() {
            let max_kid = "x".repeat(MAX_KID_LEN);
            let codec =
                RequestStateCodec::new_with_keyring(max_kid.clone(), [(max_kid, KEY_A)]).unwrap();
            let token = codec.seal(b"state");
            assert_eq!(codec.open(&token).unwrap(), b"state");

            let encoded_oversized = URL_SAFE_NO_PAD.encode("x".repeat(MAX_KID_LEN + 1));
            assert!(encoded_oversized.len() > MAX_ENCODED_KID_LEN);
            let oversized = format!("rs2.{encoded_oversized}.!!!!.!!!!");
            assert!(matches!(
                codec.open(&oversized),
                Err(RequestStateError::InvalidKeyId)
            ));

            let overlong_invalid_base64 =
                format!("rs2.{}.!!!!.!!!!", "!".repeat(MAX_ENCODED_KID_LEN + 1));
            assert!(matches!(
                codec.open(&overlong_invalid_base64),
                Err(RequestStateError::InvalidKeyId)
            ));
        }

        #[test]
        fn rs1_fallback_is_idempotent_and_rs1_signing_adds_its_key() {
            let codec = two_key_ring("b")
                .with_rs1_fallback("a")
                .unwrap()
                .with_rs1_fallback("a")
                .unwrap();
            match &codec.keys {
                Keys::Ring { rs1_fallbacks, .. } => assert_eq!(rs1_fallbacks, &["a"]),
                Keys::Single(_) => panic!("expected ring"),
            }

            let transitional = two_key_ring("b").with_rs1_signing("a").unwrap();
            match &transitional.keys {
                Keys::Ring {
                    seal_mode,
                    rs1_fallbacks,
                    ..
                } => {
                    assert!(matches!(
                        seal_mode,
                        SealMode::Rs1 { key_id } if key_id == "a"
                    ));
                    assert_eq!(rs1_fallbacks, &["a"]);
                }
                Keys::Single(_) => panic!("expected ring"),
            }
        }

        #[test]
        fn parser_is_strict_and_error_precedence_is_stable() {
            let codec = two_key_ring("a");
            for malformed in [
                "rs2",
                "rs2.YQ",
                "rs2.YQ.body",
                "rs2.YQ.body.tag.extra",
                "rs3.YQ.body.tag",
            ] {
                assert!(matches!(
                    codec.open(malformed),
                    Err(RequestStateError::MalformedFormat)
                ));
            }

            assert!(matches!(
                codec.open("rs2.!!!!.!!!!.!!!!"),
                Err(RequestStateError::InvalidEncoding)
            ));
            assert!(matches!(
                codec.open("rs2..!!!!.!!!!"),
                Err(RequestStateError::InvalidKeyId)
            ));
            let non_utf8 = URL_SAFE_NO_PAD.encode([0xff]);
            assert!(matches!(
                codec.open(&format!("rs2.{non_utf8}.!!!!.!!!!")),
                Err(RequestStateError::InvalidKeyId)
            ));

            // Key selection precedes decoding later rs2 sections.
            let unknown = URL_SAFE_NO_PAD.encode(b"missing");
            assert!(matches!(
                codec.open(&format!("rs2.{unknown}.!!!!.!!!!")),
                Err(RequestStateError::UnknownKeyId)
            ));
            assert!(matches!(
                codec.open("rs2.YQ.!!!!.!!!!"),
                Err(RequestStateError::InvalidEncoding)
            ));
            let valid_body = URL_SAFE_NO_PAD.encode(body(0, b"state"));
            assert!(matches!(
                codec.open(&format!("rs2.YQ.{valid_body}.!!!!")),
                Err(RequestStateError::InvalidEncoding)
            ));
            assert!(matches!(
                codec.open("rs2.YQ.."),
                Err(RequestStateError::IntegrityCheckFailed)
            ));

            let valid_rs2 = codec.seal(b"state");
            assert!(matches!(
                RequestStateCodec::new(KEY_A).open(&valid_rs2),
                Err(RequestStateError::UnknownKeyId)
            ));

            // With no eligible rs1 key, selection fails before body/tag decode.
            assert!(matches!(
                codec.open("rs1.!!!!.!!!!"),
                Err(RequestStateError::UnknownKeyId)
            ));
            let with_fallback = codec.with_rs1_fallback("a").unwrap();
            assert!(matches!(
                with_fallback.open("rs1.!!!!.!!!!"),
                Err(RequestStateError::InvalidEncoding)
            ));
        }

        #[test]
        fn authenticated_short_body_is_malformed() {
            let short_body = b"short";
            let tag = RequestStateCodec::mac_v2(KEY_A, b"a", b"", short_body)
                .finalize()
                .into_bytes();
            let token = raw_rs2(b"a", short_body, tag.as_slice());
            assert!(matches!(
                two_key_ring("a").open(&token),
                Err(RequestStateError::MalformedFormat)
            ));
        }

        #[test]
        fn serde_is_reached_only_after_integrity_verification() {
            let codec = two_key_ring("a");
            let invalid_json = codec.seal(b"{not-json");
            let opened: Result<serde_json::Value, _> = codec.open_json(&invalid_json);
            assert!(matches!(opened, Err(RequestStateError::Deserialization(_))));

            let valid_json = codec.seal(b"{}");
            let tampered_body = body(0, b"{not-json");
            let tampered = replace_segment(&valid_json, 2, &URL_SAFE_NO_PAD.encode(tampered_body));
            let opened: Result<serde_json::Value, _> = codec.open_json(&tampered);
            assert!(matches!(
                opened,
                Err(RequestStateError::IntegrityCheckFailed)
            ));
        }

        #[test]
        fn ring_debug_redacts_all_key_material() {
            let codec = two_key_ring("b").with_rs1_fallback("a").unwrap();
            let rendered = format!("{codec:?}");
            assert!(!rendered.contains(std::str::from_utf8(KEY_A).unwrap()));
            assert!(!rendered.contains(std::str::from_utf8(KEY_B).unwrap()));
            assert!(rendered.contains("redacted"));
            assert!(rendered.contains("Rs2"));
        }

        #[test]
        fn open_methods_do_not_panic_on_mutated_or_arbitrary_strings() {
            let codec = two_key_ring("a").with_rs1_fallback("a").unwrap();
            let valid = [
                RequestStateCodec::new(KEY_A).seal(b"state"),
                codec.seal(b"state"),
            ];
            let mut corpus = vec![
                String::new(),
                ".".to_owned(),
                "...".to_owned(),
                "rs1".to_owned(),
                "rs2".to_owned(),
                "💥".to_owned(),
                "rs2.💥...".to_owned(),
            ];

            for token in valid {
                for end in 0..=token.len() {
                    corpus.push(token[..end].to_owned());
                }
                for index in 0..token.len() {
                    let mut bytes = token.as_bytes().to_vec();
                    bytes[index] = b'!';
                    corpus.push(String::from_utf8(bytes).expect("token is ASCII"));
                }
            }

            for candidate in corpus {
                let _ = codec.open(&candidate);
                let _ = codec.open_with(&candidate, b"context");
                let _: Result<serde_json::Value, _> = codec.open_json(&candidate);
                let _: Result<serde_json::Value, _> = codec.open_json_with(&candidate, b"context");
            }
        }
    }
}
