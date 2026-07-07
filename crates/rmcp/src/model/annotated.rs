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
    pub last_modified: Option<DateTime<Utc>>,
}

impl Annotations {
    /// Creates a new Annotations instance specifically for resources
    /// optional priority, and a timestamp (defaults to now if None)
    pub fn for_resource(priority: f32, timestamp: DateTime<Utc>) -> Self {
        assert!(
            (0.0..=1.0).contains(&priority),
            "Priority {priority} must be between 0.0 and 1.0"
        );
        Annotations {
            priority: Some(priority),
            last_modified: Some(timestamp),
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

    pub fn with_timestamp(mut self, timestamp: DateTime<Utc>) -> Self {
        self.last_modified = Some(timestamp);
        self
    }

    pub fn with_timestamp_now(self) -> Self {
        self.with_timestamp(Utc::now())
    }
}
