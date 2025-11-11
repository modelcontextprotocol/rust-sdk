use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{JsonObject, Meta};

/// Task lifecycle status
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum TaskStatus {
    /// Created but not started yet
    #[default]
    Pending,
    /// Currently running
    Running,
    /// Waiting for dependencies or external input
    Waiting,
    /// Cancellation requested and in progress
    Cancelling,
    /// Completed successfully
    Succeeded,
    /// Completed with failure
    Failed,
    /// Cancelled before completion
    Cancelled,
}

/// High-level task kind. Exact set may evolve with the SEP.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum TaskKind {
    #[default]
    Generation,
    Retrieval,
    Aggregation,
    Orchestration,
    ToolCall,
    /// Custom kind identifier
    Custom(String),
}

/// Progress information for long-running tasks
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct TaskProgress {
    /// Percentage progress in the range [0.0, 100.0]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent: Option<f32>,
    /// Current stage identifier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// Human-readable status message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Arbitrary structured details, protocol-specific
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// Error information for failed tasks
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct TaskError {
    /// Machine-readable error code
    pub code: String,
    /// Human-readable error message
    pub message: String,
    /// Whether the operation can be retried safely
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    /// Arbitrary error data for debugging
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Final result for a succeeded task
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct TaskResult {
    /// MIME type or custom content-type identifier
    pub content_type: String,
    /// The actual result payload
    pub value: Value,
    /// Optional short summary for UI
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// Primary Task object used across client/server
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct Task {
    /// Unique task identifier
    pub id: String,
    /// Task kind/category
    pub kind: TaskKind,
    /// Current status
    pub status: TaskStatus,
    /// ISO8601 creation time
    pub created_at: String,
    /// ISO8601 last update time
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    /// ISO8601 start time
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    /// ISO8601 completion time
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    /// Parent task identifier for hierarchical tasks
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// List of prerequisite task ids
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    /// Optional labels for filtering and grouping
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    /// Immutable metadata provided at creation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    /// Mutable runtime state exposed to clients
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_state: Option<Value>,
    /// Input parameters for this task
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    /// Progress info when running
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<TaskProgress>,
    /// Final result when succeeded
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<TaskResult>,
    /// Error information when failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<TaskError>,
    /// True if a cancellation has been requested
    #[serde(default)]
    pub cancellation_requested: bool,
    /// Scheduling priority; larger means higher priority (convention)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
    /// Batch/group identifier for bulk operations
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_group: Option<String>,
    /// Trace identifier for observability systems
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// Protocol-level metadata for the task object
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
    /// Reserved for future SEP extensions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<JsonObject>,
}

/// Query filter for listing tasks
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct TaskQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<Vec<TaskStatus>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<Vec<TaskKind>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels_any: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels_all: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_after: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Paginated list of tasks
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct TaskList {
    pub tasks: Vec<Task>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

/// Request payload to create a new task
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct TaskCreateRequest {
    pub kind: TaskKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depends_on: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_group: Option<String>,
    /// Protocol-level metadata for the request object
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
    /// Reserved for future SEP extensions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<JsonObject>,
}

/// Request payload to update a task's runtime fields
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct TaskUpdateRequest {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_state: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancellation_requested: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
}

/// Incremental progress event for streaming updates
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct TaskProgressEvent {
    pub id: String,
    pub progress: TaskProgress,
    /// ISO8601 timestamp for the event
    pub timestamp: String,
}

/// Terminal event signaling task completion
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct TaskCompletionEvent {
    pub id: String,
    /// Allowed values: Succeeded, Failed, Cancelled
    pub status: TaskStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<TaskResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<TaskError>,
    /// ISO8601 timestamp for the event
    pub timestamp: String,
}
