//! Annotations for content blocks and resources.
//!
//! The `Annotations` struct carries optional hints about audience, priority, and freshness.
//! Individual content/resource types embed `annotations: Option<Annotations>` directly.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::Role;

/// Optional annotations for the client. The client can use annotations to inform how objects are
/// used or displayed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Annotations {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audience: Option<Vec<Role>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "lastModified")]
    pub last_modified: Option<String>,
}

impl Annotations {
    /// Creates annotations for a resource with priority and an RFC 3339 timestamp.
    ///
    /// # Panics
    ///
    /// Panics if `priority` is not in the inclusive range `0.0..=1.0`.
    pub fn for_resource(priority: f32, timestamp: DateTime<Utc>) -> Self {
        assert!(
            (0.0..=1.0).contains(&priority),
            "Priority {priority} must be between 0.0 and 1.0"
        );
        Self {
            priority: Some(priority),
            last_modified: Some(timestamp.to_rfc3339()),
            audience: None,
        }
    }

    pub fn with_audience(mut self, audience: Vec<Role>) -> Self {
        self.audience = Some(audience);
        self
    }

    pub fn with_priority(mut self, priority: f32) -> Self {
        self.priority = Some(priority);
        self
    }

    /// Sets `lastModified` from a typed timestamp, serialized as RFC 3339.
    pub fn with_timestamp(mut self, timestamp: DateTime<Utc>) -> Self {
        self.last_modified = Some(timestamp.to_rfc3339());
        self
    }

    /// Sets `lastModified` to the current time, serialized as RFC 3339.
    pub fn with_timestamp_now(self) -> Self {
        self.with_timestamp(Utc::now())
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;

    use super::*;

    fn annotations_of(value: serde_json::Value) -> Annotations {
        serde_json::from_value::<Annotations>(value).unwrap()
    }

    #[test]
    fn preserves_date_only_last_modified_verbatim() {
        let annotations = annotations_of(json!({ "lastModified": "2025-01-12" }));
        assert_eq!(annotations.last_modified.as_deref(), Some("2025-01-12"));
    }

    #[test]
    fn preserves_rfc3339_last_modified_verbatim() {
        let value = "2025-01-12T15:00:58Z";
        let annotations = annotations_of(json!({ "lastModified": value }));
        assert_eq!(annotations.last_modified.as_deref(), Some(value));
    }

    #[test]
    fn missing_last_modified_is_none() {
        let annotations = annotations_of(json!({}));
        assert_eq!(annotations.last_modified, None);
    }

    #[test]
    fn null_last_modified_is_none() {
        let annotations = annotations_of(json!({ "lastModified": null }));
        assert_eq!(annotations.last_modified, None);
    }

    #[test]
    fn invalid_last_modified_string_is_preserved() {
        let annotations = annotations_of(json!({ "lastModified": "not-a-date" }));
        assert_eq!(annotations.last_modified.as_deref(), Some("not-a-date"));
    }

    #[test]
    fn timestamp_round_trips_as_rfc3339_string() {
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 12, 15, 0, 58).unwrap();
        let annotations = Annotations::default().with_timestamp(timestamp);

        let value = serde_json::to_value(&annotations).unwrap();
        assert_eq!(value["lastModified"], json!(timestamp.to_rfc3339()));

        let round_tripped: Annotations = serde_json::from_value(value).unwrap();
        assert_eq!(round_tripped, annotations);
    }
}
