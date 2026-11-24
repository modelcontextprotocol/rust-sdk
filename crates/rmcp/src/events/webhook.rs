//! Standard Webhooks authentication for the Events Beta.

use std::fmt;

use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use http::HeaderMap;
use serde_json::{Value, json};
use sha2::Sha256;
use zeroize::Zeroizing;

use super::wire::{EventOccurrence, EventsError};
use crate::{ErrorData, model::JsonObject};

pub(super) const MAX_BODY: usize = 256 * 1024;

#[cfg(all(feature = "events-webhook", feature = "server"))]
pub(super) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0, |diff, (a, b)| diff | (a ^ b)) == 0
}

#[cfg(any(
    feature = "client",
    all(feature = "events-webhook", feature = "server")
))]
mod rpc;
#[cfg(feature = "client")]
pub use rpc::WebhookOptions;
#[cfg(any(
    feature = "client",
    all(feature = "events-webhook", feature = "server")
))]
pub(super) use rpc::*;
#[cfg(any(
    feature = "client",
    all(feature = "events-webhook", feature = "server")
))]
pub use rpc::{WebhookSubscription, WebhookSubscriptionStatus, WebhookTtl};

/// A Standard Webhooks symmetric signing key.
#[derive(Clone)]
pub struct WebhookSecret(Zeroizing<Vec<u8>>);

impl WebhookSecret {
    /// Create a key containing 32 bytes from the system-seeded CSPRNG.
    pub fn generate() -> Self {
        let mut bytes = Zeroizing::new(vec![0; 32]);
        rand::fill(bytes.as_mut_slice());
        Self(bytes)
    }

    /// Import an existing `whsec_` key containing 24 to 64 random bytes.
    pub fn from_existing(secret: &str) -> Result<Self, EventsError> {
        let invalid = || EventsError::InvalidParams("invalid webhook secret".into());
        let encoded = secret.strip_prefix("whsec_").ok_or_else(invalid)?;
        if !(32..=88).contains(&encoded.len()) {
            return Err(invalid());
        }
        let bytes = Zeroizing::new(STANDARD.decode(encoded).map_err(|_| invalid())?);
        if !(24..=64).contains(&bytes.len()) {
            return Err(invalid());
        }
        Ok(Self(bytes))
    }

    #[cfg(feature = "client")]
    pub(super) fn encode(&self) -> Zeroizing<String> {
        let mut encoded = Zeroizing::new(String::with_capacity(6 + 88));
        encoded.push_str("whsec_");
        STANDARD.encode_string(self.0.as_slice(), &mut encoded);
        encoded
    }

    #[cfg(all(feature = "events-webhook", feature = "server"))]
    pub(super) fn same_key(&self, other: &Self) -> bool {
        constant_time_eq(&self.0, &other.0)
    }

    fn mac(&self, id: &str, timestamp: &str, body: &[u8]) -> Hmac<Sha256> {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.0).expect("HMAC accepts arbitrary keys");
        mac.update(id.as_bytes());
        mac.update(b".");
        mac.update(timestamp.as_bytes());
        mac.update(b".");
        mac.update(body);
        mac
    }

    #[cfg(any(test, all(feature = "events-webhook", feature = "server")))]
    pub(super) fn signature(&self, id: &str, timestamp: &str, body: &[u8]) -> String {
        format!(
            "v1,{}",
            STANDARD.encode(self.mac(id, timestamp, body).finalize().into_bytes())
        )
    }
}

impl fmt::Debug for WebhookSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WebhookSecret([redacted])")
    }
}

/// An authenticated webhook body. The application owns routing and acceptance.
#[derive(Debug)]
#[non_exhaustive]
pub enum WebhookMessage {
    /// An event, with its event ID and safe checkpoint.
    Occurrence(EventOccurrence),
    /// Replay was truncated; reconcile from this fresh checkpoint.
    Gap {
        /// Fresh checkpoint.
        cursor: Option<String>,
    },
    /// The subscription ended on the server.
    Terminated {
        /// Termination reason.
        error: ErrorData,
    },
    /// Echo the challenge locally; do not forward it.
    Verification {
        /// The authenticated nonce.
        challenge: String,
    },
    /// An authenticated future control that can be acknowledged and ignored.
    UnknownControl,
}

/// A verified webhook and the identity information needed for safe routing.
#[derive(Debug)]
#[non_exhaustive]
pub struct VerifiedWebhook<'a> {
    /// Authenticated per-message ID, stable across retries.
    pub webhook_id: &'a str,
    /// The subscription routing header, which is **not** signed.
    pub subscription_id: &'a str,
    /// Index in the supplied candidate secret slice.
    pub matched_secret: usize,
    /// Parsed body, verified before parsing.
    pub message: WebhookMessage,
}

/// Verify a raw webhook body (at most 256 KiB) with a five-minute clock window.
///
/// The routing header is unsigned. Check that the matched secret belongs to
/// the indicated subscription before accepting a delivery. This function
/// does not store secrets, message IDs or cursors.
pub fn verify_webhook<'a>(
    headers: &'a HeaderMap,
    body: &[u8],
    secrets: &[WebhookSecret],
    now: DateTime<Utc>,
) -> Result<VerifiedWebhook<'a>, EventsError> {
    let invalid = || EventsError::InvalidParams("invalid webhook delivery".into());
    if body.len() > MAX_BODY {
        return Err(invalid());
    }
    let header = |name| {
        let mut values = headers.get_all(name).iter();
        let value = values.next()?.to_str().ok()?;
        (values.next().is_none() && !value.is_empty()).then_some(value)
    };
    let webhook_id = header("webhook-id").ok_or_else(invalid)?;
    if webhook_id.contains('.') {
        return Err(invalid());
    }
    let subscription_id = header("x-mcp-subscription-id").ok_or_else(invalid)?;
    let timestamp = header("webhook-timestamp").ok_or_else(invalid)?;
    let digits = timestamp.strip_prefix('-').unwrap_or(timestamp);
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    let seconds: i64 = timestamp.parse().map_err(|_| invalid())?;
    if now.timestamp().abs_diff(seconds) > 300 {
        return Err(invalid());
    }
    let signatures = header("webhook-signature").ok_or_else(invalid)?;
    let matched_secret = secrets
        .iter()
        .position(|secret| {
            let mac = secret.mac(webhook_id, timestamp, body);
            signatures.split_ascii_whitespace().any(|signature| {
                let mut bytes = [0; 32];
                signature.strip_prefix("v1,").is_some_and(|encoded| {
                    STANDARD.decode_slice(encoded, &mut bytes) == Ok(32)
                        && mac.clone().verify_slice(&bytes).is_ok()
                })
            })
        })
        .ok_or_else(invalid)?;
    let value: Value = serde_json::from_slice(body).map_err(|_| invalid())?;
    let message = match value.get("type") {
        None => WebhookMessage::Occurrence(serde_json::from_value(value).map_err(|_| invalid())?),
        Some(Value::String(kind)) => match kind.as_str() {
            "gap" => WebhookMessage::Gap {
                cursor: value
                    .get("cursor")
                    .map(|cursor| serde_json::from_value::<Option<String>>(cursor.clone()))
                    .transpose()
                    .map_err(|_| invalid())?
                    .flatten(),
            },
            "terminated" => WebhookMessage::Terminated {
                error: serde_json::from_value(value.get("error").ok_or_else(invalid)?.clone())
                    .map_err(|_| invalid())?,
            },
            "verification" => WebhookMessage::Verification {
                challenge: value
                    .get("challenge")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid)?
                    .to_owned(),
            },
            _ => WebhookMessage::UnknownControl,
        },
        Some(_) => return Err(invalid()),
    };
    Ok(VerifiedWebhook {
        webhook_id,
        subscription_id,
        matched_secret,
        message,
    })
}

/// Create the JSON body for a successful verification response.
pub fn challenge_response(challenge: &str) -> JsonObject {
    JsonObject::from_iter([("challenge".into(), json!(challenge))])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_webhooks_reference_vector() {
        // standard-webhooks/libraries/rust/src/lib.rs, test_sign
        let secret =
            WebhookSecret::from_existing("whsec_C2FVsBQIhrscChlQIMV+b5sSYspob7oD").unwrap();
        let signature = secret.signature(
            "msg_27UH4WbU6Z5A5EzD8u03UvzRbpk",
            "1649367553",
            br#"{"email":"test@example.com","username":"test_user"}"#,
        );
        assert_eq!(signature, "v1,tZ1I4/hDygAJgO5TYxiSd6Sd0kDW6hPenDe+bTa3Kkw=");
    }

    #[test]
    fn secret_lengths_and_redaction() {
        for len in [0, 23, 24, 32, 64, 65] {
            let encoded = format!("whsec_{}", STANDARD.encode(vec![1; len]));
            assert_eq!(
                WebhookSecret::from_existing(&encoded).is_ok(),
                (24..=64).contains(&len)
            );
        }
        assert!(WebhookSecret::from_existing("not_a_secret").is_err());
        assert!(WebhookSecret::from_existing("whsec_!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!").is_err());
        let secret = WebhookSecret::generate();
        assert_eq!(secret.0.len(), 32);
        assert_eq!(format!("{secret:?}"), "WebhookSecret([redacted])");
        let request = crate::model::CustomRequest::new(
            "events/subscribe",
            Some(json!({"delivery": {"secret": "whsec_example"}})),
        );
        assert!(!format!("{request:?}").contains("whsec_example"));
    }

    #[test]
    fn verifies_raw_bytes_with_multiple_keys_and_versions() {
        let old = WebhookSecret::generate();
        let new = WebhookSecret::generate();
        let bad = WebhookSecret::generate();
        let id = "msg_verification_example";
        let seconds = 1649367553_i64;
        let timestamp = seconds.to_string();
        let now = |offset| DateTime::from_timestamp(seconds + offset, 0).unwrap();
        let body = br#"{ "type": "verification", "challenge": "sample" }"#;
        let signature = format!(
            "v1a,unused {} {}",
            new.signature(id, &timestamp, body),
            old.signature(id, &timestamp, body),
        );
        let mut headers = HeaderMap::new();
        headers.insert("webhook-id", id.parse().unwrap());
        headers.insert("x-mcp-subscription-id", "sub_abc".parse().unwrap());
        headers.insert("webhook-timestamp", timestamp.parse().unwrap());
        headers.insert("webhook-signature", signature.parse().unwrap());
        let receipt =
            verify_webhook(&headers, body, &[bad.clone(), old.clone()], now(-300)).unwrap();
        assert_eq!(receipt.webhook_id, id);
        assert_eq!(receipt.subscription_id, "sub_abc");
        assert_eq!(receipt.matched_secret, 1);
        let WebhookMessage::Verification { challenge } = receipt.message else {
            panic!("expected challenge")
        };
        assert_eq!(
            Value::Object(challenge_response(&challenge)),
            json!({"challenge": "sample"})
        );
        assert!(verify_webhook(&headers, body, std::slice::from_ref(&new), now(300)).is_ok());
        for offset in [-301, 301] {
            assert!(
                verify_webhook(&headers, body, std::slice::from_ref(&old), now(offset)).is_err()
            );
        }
        assert!(verify_webhook(&headers, body, &[bad], now(0)).is_err());
        assert!(verify_webhook(&headers, &body[1..], &[old], now(0)).is_err());
    }

    #[test]
    fn authenticates_future_controls_and_tolerates_missing_or_null_gap_cursor() {
        let secret = WebhookSecret::generate();
        let id = "msg_control_example";
        let now = DateTime::from_timestamp(1649367553, 0).unwrap();
        let timestamp = now.timestamp().to_string();
        let mut headers = HeaderMap::new();
        headers.insert("webhook-id", id.parse().unwrap());
        headers.insert("x-mcp-subscription-id", "sub_abc".parse().unwrap());
        headers.insert("webhook-timestamp", timestamp.parse().unwrap());
        for (body, unknown) in [
            (br#"{"type":"future"}"#.as_slice(), true),
            (br#"{"type":"gap"}"#.as_slice(), false),
            (br#"{"type":"gap","cursor":null}"#.as_slice(), false),
        ] {
            headers.insert(
                "webhook-signature",
                secret.signature(id, &timestamp, body).parse().unwrap(),
            );
            let got = verify_webhook(&headers, body, std::slice::from_ref(&secret), now).unwrap();
            if unknown {
                assert!(matches!(got.message, WebhookMessage::UnknownControl));
            } else {
                assert!(matches!(got.message, WebhookMessage::Gap { cursor: None }));
            }
        }
    }
}
