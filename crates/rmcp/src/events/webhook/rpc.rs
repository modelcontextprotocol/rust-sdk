use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::Zeroizing;

#[cfg(feature = "client")]
use super::WebhookSecret;
use crate::model::{JsonObject, ResultType};

pub(in crate::events) const SUBSCRIBE: &str = "events/subscribe";
pub(in crate::events) const UNSUBSCRIBE: &str = "events/unsubscribe";

/// Lifetime suggested to the webhook server.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum WebhookTtl {
    /// Omit the suggestion and use the server's default.
    #[default]
    Default,
    /// Request a grant without expiry. The server can grant a finite lifetime.
    NoExpiry,
    /// Suggest a finite lifetime, in milliseconds.
    Milliseconds(u64),
}

impl WebhookTtl {
    fn is_default(&self) -> bool {
        *self == Self::Default
    }
}

impl Serialize for WebhookTtl {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Milliseconds(ms) => serializer.serialize_u64(*ms),
            _ => serializer.serialize_none(),
        }
    }
}

impl<'de> Deserialize<'de> for WebhookTtl {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Option::<u64>::deserialize(deserializer)?.map_or(Self::NoExpiry, Self::Milliseconds))
    }
}

/// Inputs for subscribing or refreshing a webhook.
#[cfg(feature = "client")]
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct WebhookOptions {
    /// HTTPS callback URL.
    pub url: String,
    /// Key the endpoint expects. Supply a replacement here to rotate it.
    pub secret: WebhookSecret,
    /// Desired grant; the response deadline is authoritative.
    pub ttl: WebhookTtl,
    /// Last safe checkpoint, or `None` to start from now.
    pub cursor: Option<String>,
    /// Optional age floor for a resumed grant.
    pub max_age_ms: Option<u64>,
}

#[cfg(feature = "client")]
impl WebhookOptions {
    /// Start from now with the server's default lifetime.
    pub fn new(url: impl Into<String>, secret: WebhookSecret) -> Self {
        Self {
            url: url.into(),
            secret,
            ttl: WebhookTtl::Default,
            cursor: None,
            max_age_ms: None,
        }
    }
}

/// The authoritative webhook grant returned by a subscribe or refresh.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct WebhookSubscription {
    #[serde(default, deserialize_with = "crate::events::wire::complete")]
    pub(in crate::events) result_type: ResultType,
    /// Stable server-derived routing handle (not a credential).
    pub id: String,
    /// RFC 3339 expiry. `None` is a grant with no expiry.
    #[serde(deserialize_with = "read_optional_timestamp")]
    pub refresh_before: Option<String>,
    /// Latest safe checkpoint. Do not persist a null checkpoint.
    pub cursor: Option<String>,
    /// The server had to start after the requested checkpoint.
    #[serde(default)]
    pub truncated: bool,
    /// Optional delivery health snapshot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_status: Option<WebhookSubscriptionStatus>,
}

/// Delivery health reported by the webhook server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct WebhookSubscriptionStatus {
    /// Whether delivery is active.
    pub active: bool,
    /// RFC 3339 time of the last successful acknowledgement.
    #[serde(default, deserialize_with = "read_optional_timestamp")]
    pub last_delivery_at: Option<String>,
    /// Fixed error category, not endpoint response content.
    pub last_error: Option<String>,
}

fn read_optional_timestamp<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let timestamp = Option::<String>::deserialize(deserializer)?;
    if let Some(value) = &timestamp {
        // These fields are passed through, not compared or normalized.
        crate::events::wire::timestamp_offset(value).map_err(serde::de::Error::custom)?;
    }
    Ok(timestamp)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::events) struct SubscribeParams {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<JsonObject>,
    pub delivery: SubscribeDelivery,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_age_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "WebhookTtl::is_default")]
    pub ttl_ms: WebhookTtl,
}

#[derive(Serialize, Deserialize)]
pub(in crate::events) struct SubscribeDelivery {
    pub mode: SubscribeMode,
    pub url: String,
    pub secret: Zeroizing<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(in crate::events) enum SubscribeMode {
    Webhook,
}

#[derive(Serialize, Deserialize)]
pub(in crate::events) struct UnsubscribeParams {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<JsonObject>,
    pub delivery: UnsubscribeDelivery,
}

#[derive(Serialize, Deserialize)]
pub(in crate::events) struct UnsubscribeDelivery {
    pub url: String,
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    #[test]
    fn omitted_null_and_finite_ttl_remain_distinct() {
        let mut request = json!({
            "name": "example",
            "delivery": {"mode": "webhook", "url": "https://example.net", "secret": "fixture"}
        });
        let omitted: SubscribeParams = serde_json::from_value(request.clone()).unwrap();
        assert_eq!(omitted.ttl_ms, WebhookTtl::Default);
        assert!(
            serde_json::to_value(omitted)
                .unwrap()
                .get("ttlMs")
                .is_none()
        );
        for (value, hint) in [
            (Value::Null, WebhookTtl::NoExpiry),
            (json!(123), WebhookTtl::Milliseconds(123)),
        ] {
            request["ttlMs"] = value.clone();
            let params: SubscribeParams = serde_json::from_value(request.clone()).unwrap();
            assert_eq!(params.ttl_ms, hint);
            assert_eq!(serde_json::to_value(params).unwrap()["ttlMs"], value);
        }
    }

    #[test]
    fn missing_result_type_means_complete_but_grant_deadline_is_required() {
        let mut wire = json!({"id": "sub_fixture", "refreshBefore": null});
        let result: WebhookSubscription = serde_json::from_value(wire.clone()).unwrap();
        assert!(result.result_type.is_complete());
        assert!(result.refresh_before.is_none());
        let deadline = "9999-12-31T23:59:59Z";
        let finite: WebhookSubscription = serde_json::from_value(json!({
            "id": "sub_fixture", "refreshBefore": deadline, "deliveryStatus": {"active": true}
        }))
        .unwrap();
        assert_eq!(finite.refresh_before.as_deref(), Some(deadline));
        assert!(finite.delivery_status.unwrap().last_delivery_at.is_none());
        let status: WebhookSubscriptionStatus = serde_json::from_value(json!({
            "active": true, "lastDeliveryAt": deadline
        }))
        .unwrap();
        assert_eq!(status.last_delivery_at.as_deref(), Some(deadline));
        assert!(
            serde_json::from_value::<WebhookSubscription>(
                json!({"id":"sub_fixture", "refreshBefore":"not-a-timestamp"})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<WebhookSubscription>(json!({
                "id":"sub_fixture", "refreshBefore": deadline,
                "deliveryStatus": {"active": true, "lastDeliveryAt": "not-a-timestamp"}
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<WebhookSubscription>(
                json!({"id":"sub_fixture", "refreshBefore":null, "resultType":"task"})
            )
            .is_err()
        );
        wire.as_object_mut().unwrap().remove("refreshBefore");
        assert!(serde_json::from_value::<WebhookSubscription>(wire).is_err());
    }
}
