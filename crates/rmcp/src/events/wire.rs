//! Catalog, poll, and error types for the Events Beta.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::{ErrorCode, ErrorData, JsonObject};

/// Delivery mode advertised for an event type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum DeliveryMode {
    /// One-shot cursor-based reads.
    Poll,
    /// An active transport stream.
    Push,
    /// Delivery to a registered HTTPS endpoint.
    Webhook,
}

/// A wire catalog entry, separate from the server's registration policy.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct EventDescriptor {
    /// Event type name.
    pub name: String,
    /// Human-readable description, empty when omitted by the peer.
    #[serde(default)]
    pub description: String,
    /// Recognized modes. An unknown-only peer entry has an empty list here.
    #[serde(deserialize_with = "read_delivery")]
    pub delivery: Vec<DeliveryMode>,
    /// Subscription argument schema.
    pub input_schema: Arc<JsonObject>,
    /// Occurrence data schema.
    pub payload_schema: Arc<JsonObject>,
    /// Optional catalog metadata.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<JsonObject>,
}

impl EventDescriptor {
    /// Construct a wire descriptor for use with a raw server registration.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        delivery: Vec<DeliveryMode>,
        input_schema: Arc<JsonObject>,
        payload_schema: Arc<JsonObject>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            delivery,
            input_schema,
            payload_schema,
            meta: None,
        }
    }
}

fn read_delivery<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<DeliveryMode>, D::Error> {
    let modes = Vec::<String>::deserialize(deserializer)?;
    if modes.is_empty() {
        return Err(serde::de::Error::custom("empty Events delivery"));
    }
    Ok(modes
        .into_iter()
        .filter_map(|mode| match mode.as_str() {
            "poll" => Some(DeliveryMode::Poll),
            "push" => Some(DeliveryMode::Push),
            "webhook" => Some(DeliveryMode::Webhook),
            _ => None,
        })
        .collect())
}

/// One upstream event delivered to a subscriber.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct EventOccurrence {
    /// Stable identifier used for deduplication within a subscription.
    pub event_id: String,
    /// Event type name.
    pub name: String,
    /// When the event occurred, in UTC.
    #[serde(serialize_with = "utc_timestamp")]
    pub timestamp: DateTime<Utc>,
    /// Object payload matching the advertised schema.
    pub data: JsonObject,
    /// Optional occurrence metadata.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<JsonObject>,
    /// Optional position after this event. Poll replies carry a page cursor instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

impl EventOccurrence {
    /// Construct an occurrence without item cursor or metadata.
    pub fn new(
        event_id: impl Into<String>,
        name: impl Into<String>,
        timestamp: DateTime<Utc>,
        data: JsonObject,
    ) -> Self {
        Self {
            event_id: event_id.into(),
            name: name.into(),
            timestamp,
            data,
            meta: None,
            cursor: None,
        }
    }
}

fn utc_timestamp<S: serde::Serializer>(
    time: &DateTime<Utc>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&time.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true))
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum Completion {
    Complete,
}

/// One page of a peer's catalog.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ListEventsResult {
    pub(super) result_type: Completion,
    /// Catalog entries on this page.
    #[cfg_attr(feature = "client", serde(deserialize_with = "read_catalog"))]
    pub events: Vec<EventDescriptor>,
    /// Opaque catalog cursor; absent at the last page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// Optional result metadata.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<JsonObject>,
}

#[cfg(feature = "client")]
fn read_catalog<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<EventDescriptor>, D::Error> {
    Ok(Vec::<Value>::deserialize(deserializer)?
        .into_iter()
        .filter_map(|item| {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>")
                .to_owned();
            match serde_json::from_value(item) {
                Ok(descriptor) => Some(descriptor),
                Err(_) => {
                    tracing::warn!(event = %name, "ignoring malformed event descriptor");
                    None
                }
            }
        })
        .collect())
}

/// Arguments for a one-shot cursor-based event read.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct PollParams {
    /// Event type to read.
    pub name: String,
    /// Subscription arguments; absence means an empty object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<JsonObject>,
    /// Saved replay position; absence starts at the current head.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Optional replay age limit, in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_age_ms: Option<u64>,
    /// Optional positive cap on returned occurrences.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_events: Option<u64>,
}

impl PollParams {
    /// Start from the current head with empty arguments and server defaults.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            arguments: None,
            cursor: None,
            max_age_ms: None,
            max_events: None,
        }
    }
}

pub(super) const DEFAULT_POLL_MS: u64 = 30_000;
fn default_poll_ms() -> u64 {
    DEFAULT_POLL_MS
}

/// One page of events and its checkpoint.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct PollResult {
    pub(super) result_type: Completion,
    /// Occurrences in delivery order. An unparseable occurrence rejects the
    /// entire page, including its checkpoint.
    pub events: Vec<EventOccurrence>,
    /// Checkpoint to persist after consuming this page, if supplied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Whether replay advanced across a gap.
    #[serde(default)]
    pub truncated: bool,
    /// Whether more work remains after this page.
    #[serde(default)]
    pub has_more: bool,
    /// Suggested delay after draining the backlog, in milliseconds.
    #[serde(default = "default_poll_ms")]
    pub next_poll_ms: u64,
    /// Optional result metadata.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<JsonObject>,
}

const NOT_FOUND: ErrorCode = ErrorCode(-32023);
const FORBIDDEN: ErrorCode = ErrorCode(-32024);
const RESOURCE_EXHAUSTED: ErrorCode = ErrorCode(-32025);
const UNSUPPORTED: ErrorCode = ErrorCode(-32026);

/// Local, remote, or transport failure from an Events operation.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EventsError {
    /// Events or the requested method is unavailable.
    #[error("Events method unavailable")]
    MethodNotFound,
    /// Invalid request parameters or event data.
    #[error("{0}")]
    InvalidParams(String),
    /// Routing headers do not match the request body.
    #[error("{0}")]
    HeaderMismatch(String),
    /// The event type or subscription does not exist.
    #[error("event or subscription not found")]
    NotFound {
        /// Optional discriminator (`event` or `subscription`).
        kind: Option<String>,
    },
    /// The caller is not authorized.
    #[error("Events access forbidden")]
    Forbidden {
        /// Optional safe-to-expose reason.
        reason: Option<String>,
    },
    /// A source or server reached a fixed limit.
    #[error("Events resource limit exceeded: {limit}")]
    ResourceExhausted {
        /// Name of the limited resource.
        limit: String,
        /// Maximum supported value, if available.
        max: Option<u64>,
    },
    /// An operation or schema is not supported.
    #[error("unsupported Events feature: {feature}")]
    Unsupported {
        /// Wire feature or schema that is unsupported.
        feature: String,
        /// Requested value, if available.
        value: Option<Value>,
        /// Optional machine-readable reason.
        reason: Option<String>,
    },
    /// Preserve the original JSON-RPC error.
    #[error("{0}")]
    Remote(Box<ErrorData>),
    /// Failure in the client service or transport.
    #[cfg(feature = "client")]
    #[error(transparent)]
    Service(Box<crate::ServiceError>),
    /// The peer sent the wrong result family or malformed contents.
    #[cfg(feature = "client")]
    #[error("invalid Events response")]
    UnexpectedResponse,
}

impl From<EventsError> for ErrorData {
    fn from(error: EventsError) -> Self {
        let message = error.to_string();
        let (code, data) = match error {
            EventsError::Remote(error) => return *error,
            EventsError::MethodNotFound => (ErrorCode::METHOD_NOT_FOUND, None),
            EventsError::InvalidParams(_) => (ErrorCode::INVALID_PARAMS, None),
            EventsError::HeaderMismatch(_) => (ErrorCode::HEADER_MISMATCH, None),
            EventsError::NotFound { kind } => (NOT_FOUND, kind.map(|kind| json!({ "kind": kind }))),
            EventsError::Forbidden { reason } => {
                (FORBIDDEN, reason.map(|reason| json!({ "reason": reason })))
            }
            EventsError::ResourceExhausted { limit, max } => (
                RESOURCE_EXHAUSTED,
                Some(max.map_or_else(
                    || json!({ "limit": limit }),
                    |max| json!({ "limit": limit, "max": max }),
                )),
            ),
            EventsError::Unsupported {
                feature,
                value,
                reason,
            } => {
                let mut data = JsonObject::new();
                data.insert("feature".into(), Value::String(feature));
                if let Some(value) = value {
                    data.insert("value".into(), value);
                }
                if let Some(reason) = reason {
                    data.insert("reason".into(), Value::String(reason));
                }
                (UNSUPPORTED, Some(Value::Object(data)))
            }
            #[cfg(feature = "client")]
            _ => (ErrorCode::INTERNAL_ERROR, None),
        };
        Self::new(code, message, data)
    }
}

impl From<ErrorData> for EventsError {
    fn from(error: ErrorData) -> Self {
        let get = |key| error.data.as_ref().and_then(|data| data.get(key));
        let string = |key| get(key).and_then(Value::as_str).map(str::to_owned);
        match error.code {
            ErrorCode::METHOD_NOT_FOUND => Self::MethodNotFound,
            ErrorCode::INVALID_PARAMS => Self::InvalidParams(error.message.into_owned()),
            ErrorCode::HEADER_MISMATCH => Self::HeaderMismatch(error.message.into_owned()),
            NOT_FOUND => Self::NotFound {
                kind: string("kind"),
            },
            FORBIDDEN => Self::Forbidden {
                reason: string("reason"),
            },
            RESOURCE_EXHAUSTED => match string("limit") {
                Some(limit) => Self::ResourceExhausted {
                    limit,
                    max: get("max").and_then(Value::as_u64),
                },
                None => Self::Remote(Box::new(error)),
            },
            UNSUPPORTED => match string("feature") {
                Some(feature) => Self::Unsupported {
                    feature,
                    value: get("value").cloned(),
                    reason: string("reason"),
                },
                None => Self::Remote(Box::new(error)),
            },
            _ => Self::Remote(Box::new(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_defaults_and_completion() {
        let params: PollParams =
            serde_json::from_value(json!({"name":"event","cursor":null})).unwrap();
        assert!(params.cursor.is_none());
        assert!(
            serde_json::to_value(params)
                .unwrap()
                .get("cursor")
                .is_none()
        );
        let result: PollResult = serde_json::from_value(json!({
            "resultType":"complete","events":[],"cursor":null
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            json!({
                "resultType":"complete","events":[],"truncated":false,"hasMore":false,"nextPollMs":30000
            })
        );
        assert!(serde_json::from_value::<PollResult>(json!({"events":[]})).is_err());
        assert!(
            serde_json::from_value::<PollResult>(json!({
                "resultType":"complete","events":[],"hasMore":null
            }))
            .is_err()
        );
    }

    #[test]
    fn catalog_requires_object_schemas_and_ignores_unknown_modes() {
        let descriptor: EventDescriptor = serde_json::from_value(json!({
            "name":"x","delivery":["future","poll"],"inputSchema":{},"payloadSchema":{}
        }))
        .unwrap();
        assert_eq!(descriptor.delivery, [DeliveryMode::Poll]);
        assert_eq!(serde_json::to_value(descriptor).unwrap()["description"], "");
        for value in [
            json!({"name":"x","delivery":[],"inputSchema":{},"payloadSchema":{}}),
            json!({"name":"x","delivery":["poll"],"inputSchema":false,"payloadSchema":{}}),
            json!({"name":"x","delivery":["poll"],"inputSchema":{}}),
        ] {
            assert!(serde_json::from_value::<EventDescriptor>(value).is_err());
        }
    }

    #[cfg(feature = "client")]
    #[test]
    fn malformed_timestamp_rejects_the_page_and_its_checkpoint() {
        let occurrence =
            |id, timestamp| json!({"eventId": id,"name":"x","timestamp":timestamp,"data":{}});
        let result = serde_json::from_value::<PollResult>(json!({
            "resultType":"complete","cursor":"resume","events":[
                occurrence("iso-basic","20261124T120000Z"), occurrence("good","2026-11-24T12:00:00Z")
            ]
        }));
        assert!(
            result.is_err(),
            "must not expose a cursor past an unparsed occurrence: {result:?}"
        );
    }

    #[test]
    fn errors_preserve_codes_and_data() {
        for (code, data) in [
            (NOT_FOUND, json!({"kind":"event"})),
            (FORBIDDEN, json!({"reason":"revoked"})),
            (RESOURCE_EXHAUSTED, json!({"limit":"eventBytes","max":5})),
            (
                UNSUPPORTED,
                json!({"feature":"inputSchema","reason":"schema_changed"}),
            ),
            (ErrorCode(-32085), json!([5, 2])),
        ] {
            let original = ErrorData::new(code, "error", Some(data.clone()));
            let decoded = ErrorData::from(EventsError::from(original));
            assert_eq!(decoded.code, code);
            assert_eq!(decoded.data, Some(data));
        }
        let invalid = ErrorData::new(ErrorCode::INVALID_PARAMS, "invalid request", None);
        assert_eq!(EventsError::from(invalid).to_string(), "invalid request");
    }

    #[test]
    fn events_debug_elides_only_events_traffic() {
        use crate::model::{CustomNotification, CustomRequest};
        let secret = Some(json!({"secret":"not in logs"}));
        assert!(
            !format!("{:?}", CustomRequest::new("events/poll", secret.clone()))
                .contains("not in logs")
        );
        assert!(
            !format!(
                "{:?}",
                CustomNotification::new("notifications/events/event", secret.clone())
            )
            .contains("not in logs")
        );
        assert!(
            format!("{:?}", CustomRequest::new("custom/method", secret)).contains("not in logs")
        );
    }
}
