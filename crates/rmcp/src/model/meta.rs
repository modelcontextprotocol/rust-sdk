use std::ops::{Deref, DerefMut};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    ClientCapabilities, ClientNotification, ClientRequest, CustomNotification, CustomRequest,
    Extensions, Implementation, JsonObject, JsonRpcMessage, LoggingLevel, NumberOrString,
    ProgressToken, ProtocolVersion, ServerNotification, ServerRequest, TaskMetadata,
};

pub trait GetMeta {
    fn get_meta_mut(&mut self) -> &mut Meta;
    fn get_meta(&self) -> &Meta;
}

pub trait GetExtensions {
    fn extensions(&self) -> &Extensions;
    fn extensions_mut(&mut self) -> &mut Extensions;
}

/// Trait for request params that contain the `_meta` field.
///
/// Per the MCP 2025-11-25 spec, all request params should have an optional `_meta`
/// field that can contain a `progressToken` for tracking long-running operations.
pub trait RequestParamsMeta {
    /// Get a reference to the meta field
    fn meta(&self) -> Option<&Meta>;
    /// Get a mutable reference to the meta field
    fn meta_mut(&mut self) -> &mut Option<Meta>;
    /// Set the meta field
    fn set_meta(&mut self, meta: Meta) {
        *self.meta_mut() = Some(meta);
    }
    /// Get the progress token from meta, if present
    fn progress_token(&self) -> Option<ProgressToken> {
        self.meta().and_then(|m| m.get_progress_token())
    }
    /// Set a progress token in meta
    fn set_progress_token(&mut self, token: ProgressToken) {
        match self.meta_mut() {
            Some(meta) => meta.set_progress_token(token),
            none => {
                let mut meta = Meta::new();
                meta.set_progress_token(token);
                *none = Some(meta);
            }
        }
    }
    /// Get the W3C `traceparent` value from meta, if present (SEP-414)
    fn traceparent(&self) -> Option<&str> {
        self.meta().and_then(|m| m.get_traceparent())
    }
    /// Set the W3C `traceparent` value in meta (SEP-414)
    fn set_traceparent(&mut self, value: &str) {
        self.meta_or_default().set_traceparent(value);
    }
    /// Get the W3C `tracestate` value from meta, if present (SEP-414)
    fn tracestate(&self) -> Option<&str> {
        self.meta().and_then(|m| m.get_tracestate())
    }
    /// Set the W3C `tracestate` value in meta (SEP-414)
    fn set_tracestate(&mut self, value: &str) {
        self.meta_or_default().set_tracestate(value);
    }
    /// Get the W3C `baggage` value from meta, if present (SEP-414)
    fn baggage(&self) -> Option<&str> {
        self.meta().and_then(|m| m.get_baggage())
    }
    /// Set the W3C `baggage` value in meta (SEP-414)
    fn set_baggage(&mut self, value: &str) {
        self.meta_or_default().set_baggage(value);
    }
    /// Get a mutable reference to meta, inserting an empty one if absent.
    fn meta_or_default(&mut self) -> &mut Meta {
        self.meta_mut().get_or_insert_with(Meta::new)
    }
}

/// Trait for task-augmented request params that contain both `_meta` and `task` fields.
///
/// Per the MCP 2025-11-25 spec, certain requests (like `tools/call` and `sampling/createMessage`)
/// can include a `task` field to signal that the caller wants task-augmented execution.
pub trait TaskAugmentedRequestParamsMeta: RequestParamsMeta {
    /// Get a reference to the task field
    fn task(&self) -> Option<&TaskMetadata>;
    /// Get a mutable reference to the task field
    fn task_mut(&mut self) -> &mut Option<TaskMetadata>;
    /// Set the task field
    fn set_task(&mut self, task: TaskMetadata) {
        *self.task_mut() = Some(task);
    }
}

impl GetExtensions for CustomNotification {
    fn extensions(&self) -> &Extensions {
        &self.extensions
    }
    fn extensions_mut(&mut self) -> &mut Extensions {
        &mut self.extensions
    }
}

impl GetMeta for CustomNotification {
    fn get_meta_mut(&mut self) -> &mut Meta {
        self.extensions_mut().get_or_insert_default()
    }
    fn get_meta(&self) -> &Meta {
        self.extensions()
            .get::<Meta>()
            .unwrap_or(Meta::static_empty())
    }
}

impl GetExtensions for CustomRequest {
    fn extensions(&self) -> &Extensions {
        &self.extensions
    }
    fn extensions_mut(&mut self) -> &mut Extensions {
        &mut self.extensions
    }
}

impl GetMeta for CustomRequest {
    fn get_meta_mut(&mut self) -> &mut Meta {
        self.extensions_mut().get_or_insert_default()
    }
    fn get_meta(&self) -> &Meta {
        self.extensions()
            .get::<Meta>()
            .unwrap_or(Meta::static_empty())
    }
}

macro_rules! variant_extension {
    (
        $Enum: ident {
            $($variant: ident)*
        }
    ) => {
        impl GetExtensions for $Enum {
            fn extensions(&self) -> &Extensions {
                match self {
                    $(
                        $Enum::$variant(v) => &v.extensions,
                    )*
                }
            }
            fn extensions_mut(&mut self) -> &mut Extensions {
                match self {
                    $(
                        $Enum::$variant(v) => &mut v.extensions,
                    )*
                }
            }
        }
        impl GetMeta for $Enum {
            fn get_meta_mut(&mut self) -> &mut Meta {
                self.extensions_mut().get_or_insert_default()
            }
            fn get_meta(&self) -> &Meta {
                self.extensions().get::<Meta>().unwrap_or(Meta::static_empty())
            }
        }
    };
}

variant_extension! {
    ClientRequest {
        PingRequest
        InitializeRequest
        CompleteRequest
        SetLevelRequest
        GetPromptRequest
        ListPromptsRequest
        ListResourcesRequest
        ListResourceTemplatesRequest
        ReadResourceRequest
        SubscribeRequest
        UnsubscribeRequest
        CallToolRequest
        ListToolsRequest
        CustomRequest
        GetTaskRequest
        ListTasksRequest
        GetTaskPayloadRequest
        CancelTaskRequest
    }
}

variant_extension! {
    ServerRequest {
        PingRequest
        CreateMessageRequest
        ListRootsRequest
        ElicitRequest
        CustomRequest
    }
}

variant_extension! {
    ClientNotification {
        CancelledNotification
        ProgressNotification
        InitializedNotification
        RootsListChangedNotification
        TaskStatusNotification
        CustomNotification
    }
}

variant_extension! {
    ServerNotification {
        CancelledNotification
        ProgressNotification
        LoggingMessageNotification
        ResourceUpdatedNotification
        ResourceListChangedNotification
        ToolListChangedNotification
        PromptListChangedNotification
        TaskStatusNotification
        CustomNotification
    }
}
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(transparent)]
#[expect(clippy::exhaustive_structs, reason = "intentionally exhaustive")]
pub struct Meta(pub JsonObject);

impl Meta {
    const PROGRESS_TOKEN_FIELD: &str = "progressToken";
    const META_KEY_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
    const META_KEY_CLIENT_INFO: &str = "io.modelcontextprotocol/clientInfo";
    const META_KEY_CLIENT_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
    const META_KEY_LOG_LEVEL: &str = "io.modelcontextprotocol/logLevel";
    /// Reserved `_meta` key for the W3C Trace Context `traceparent` value (SEP-414).
    const TRACEPARENT_FIELD: &str = "traceparent";
    /// Reserved `_meta` key for the W3C Trace Context `tracestate` value (SEP-414).
    const TRACESTATE_FIELD: &str = "tracestate";
    /// Reserved `_meta` key for the W3C Baggage value (SEP-414).
    const BAGGAGE_FIELD: &str = "baggage";

    pub fn new() -> Self {
        Self(JsonObject::new())
    }

    /// Create a new Meta with a progress token set
    pub fn with_progress_token(token: ProgressToken) -> Self {
        let mut meta = Self::new();
        meta.set_progress_token(token);
        meta
    }

    pub(crate) fn static_empty() -> &'static Self {
        static EMPTY: std::sync::OnceLock<Meta> = std::sync::OnceLock::new();
        EMPTY.get_or_init(Default::default)
    }

    pub fn get_progress_token(&self) -> Option<ProgressToken> {
        self.0
            .get(Self::PROGRESS_TOKEN_FIELD)
            .and_then(|v| match v {
                Value::String(s) => {
                    Some(ProgressToken(NumberOrString::String(s.to_string().into())))
                }
                Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        Some(ProgressToken(NumberOrString::Number(i)))
                    } else if let Some(u) = n.as_u64() {
                        if u <= i64::MAX as u64 {
                            Some(ProgressToken(NumberOrString::Number(u as i64)))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
                _ => None,
            })
    }

    pub fn set_progress_token(&mut self, token: ProgressToken) {
        match token.0 {
            NumberOrString::String(ref s) => self.0.insert(
                Self::PROGRESS_TOKEN_FIELD.to_string(),
                Value::String(s.to_string()),
            ),
            NumberOrString::Number(n) => self.0.insert(
                Self::PROGRESS_TOKEN_FIELD.to_string(),
                Value::Number(n.into()),
            ),
        };
    }

    /// Get the MCP protocol version carried in `_meta`, if present and valid.
    pub fn protocol_version(&self) -> Option<ProtocolVersion> {
        self.decode_value(Self::META_KEY_PROTOCOL_VERSION)
    }

    /// Set the MCP protocol version carried in `_meta`.
    pub fn set_protocol_version(&mut self, protocol_version: ProtocolVersion) {
        self.0.insert(
            Self::META_KEY_PROTOCOL_VERSION.to_string(),
            Value::String(protocol_version.to_string()),
        );
    }

    /// Get the client implementation identity carried in `_meta`, if present and valid.
    pub fn client_info(&self) -> Option<Implementation> {
        self.decode_value(Self::META_KEY_CLIENT_INFO)
    }

    /// Set the client implementation identity carried in `_meta`.
    pub fn set_client_info(&mut self, client_info: Implementation) {
        self.insert_serialized(Self::META_KEY_CLIENT_INFO, client_info);
    }

    /// Get the client capabilities carried in `_meta`, if present and valid.
    pub fn client_capabilities(&self) -> Option<ClientCapabilities> {
        self.decode_value(Self::META_KEY_CLIENT_CAPABILITIES)
    }

    /// Set the client capabilities carried in `_meta`.
    pub fn set_client_capabilities(&mut self, client_capabilities: ClientCapabilities) {
        self.insert_serialized(Self::META_KEY_CLIENT_CAPABILITIES, client_capabilities);
    }

    /// Get the requested per-request log level carried in `_meta`, if present and valid.
    pub fn log_level(&self) -> Option<LoggingLevel> {
        self.decode_value(Self::META_KEY_LOG_LEVEL)
    }

    /// Set the requested per-request log level carried in `_meta`.
    pub fn set_log_level(&mut self, log_level: LoggingLevel) {
        self.insert_serialized(Self::META_KEY_LOG_LEVEL, log_level);
    }

    /// Read a string-valued `_meta` field, or `None` if absent or not a string.
    fn get_str(&self, field: &str) -> Option<&str> {
        self.0.get(field).and_then(Value::as_str)
    }

    /// Write a string-valued `_meta` field.
    fn set_str(&mut self, field: &str, value: impl Into<String>) {
        self.0
            .insert(field.to_string(), Value::String(value.into()));
    }

    /// Get the W3C `traceparent` value (SEP-414), if present.
    pub fn get_traceparent(&self) -> Option<&str> {
        self.get_str(Self::TRACEPARENT_FIELD)
    }

    /// Set the W3C `traceparent` value (SEP-414).
    ///
    /// ```
    /// use rmcp::model::Meta;
    ///
    /// let mut meta = Meta::new();
    /// meta.set_traceparent("00-0af7651916cd43dd8448eb211c80319c-00f067aa0ba902b7-01");
    /// assert_eq!(
    ///     meta.get_traceparent(),
    ///     Some("00-0af7651916cd43dd8448eb211c80319c-00f067aa0ba902b7-01"),
    /// );
    /// ```
    pub fn set_traceparent(&mut self, value: impl Into<String>) {
        self.set_str(Self::TRACEPARENT_FIELD, value);
    }

    /// Get the W3C `tracestate` value (SEP-414), if present.
    pub fn get_tracestate(&self) -> Option<&str> {
        self.get_str(Self::TRACESTATE_FIELD)
    }

    /// Set the W3C `tracestate` value (SEP-414).
    pub fn set_tracestate(&mut self, value: impl Into<String>) {
        self.set_str(Self::TRACESTATE_FIELD, value);
    }

    /// Get the W3C `baggage` value (SEP-414), if present.
    pub fn get_baggage(&self) -> Option<&str> {
        self.get_str(Self::BAGGAGE_FIELD)
    }

    /// Set the W3C `baggage` value (SEP-414).
    pub fn set_baggage(&mut self, value: impl Into<String>) {
        self.set_str(Self::BAGGAGE_FIELD, value);
    }

    pub fn extend(&mut self, other: Meta) {
        for (k, v) in other.0.into_iter() {
            self.0.insert(k, v);
        }
    }

    fn decode_value<T>(&self, key: &str) -> Option<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        self.0.get(key).and_then(|value| T::deserialize(value).ok())
    }

    fn insert_serialized<T>(&mut self, key: &str, value: T)
    where
        T: Serialize,
    {
        let value = serde_json::to_value(value)
            .expect("MCP meta helper value should serialize to valid JSON");
        self.0.insert(key.to_string(), value);
    }
}

impl Deref for Meta {
    type Target = JsonObject;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Meta {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<Req, Resp, Noti> JsonRpcMessage<Req, Resp, Noti>
where
    Req: GetExtensions,
    Noti: GetExtensions,
{
    pub fn insert_extension<T: Clone + Send + Sync + 'static>(&mut self, value: T) {
        match self {
            JsonRpcMessage::Request(json_rpc_request) => {
                json_rpc_request.request.extensions_mut().insert(value);
            }
            JsonRpcMessage::Notification(json_rpc_notification) => {
                json_rpc_notification
                    .notification
                    .extensions_mut()
                    .insert(value);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Params {
        meta: Option<Meta>,
    }

    impl RequestParamsMeta for Params {
        fn meta(&self) -> Option<&Meta> {
            self.meta.as_ref()
        }
        fn meta_mut(&mut self) -> &mut Option<Meta> {
            &mut self.meta
        }
    }

    const TRACEPARENT: &str = "00-0af7651916cd43dd8448eb211c80319c-00f067aa0ba902b7-01";

    #[test]
    fn trace_context_round_trip() {
        let mut meta = Meta::new();
        meta.set_traceparent(TRACEPARENT);
        meta.set_tracestate("vendor1=value1,vendor2=value2");
        meta.set_baggage("userId=alice,region=us-east-1");
        assert_eq!(meta.get_traceparent(), Some(TRACEPARENT));
        assert_eq!(meta.get_tracestate(), Some("vendor1=value1,vendor2=value2"));
        assert_eq!(meta.get_baggage(), Some("userId=alice,region=us-east-1"));
    }

    #[test]
    fn absent_field_is_none() {
        let meta = Meta::new();
        assert_eq!(meta.get_traceparent(), None);
        assert_eq!(meta.get_tracestate(), None);
        assert_eq!(meta.get_baggage(), None);
    }

    #[test]
    fn non_string_value_is_none() {
        let mut meta = Meta::new();
        meta.0
            .insert(Meta::TRACEPARENT_FIELD.to_string(), Value::from(42));
        assert_eq!(meta.get_traceparent(), None);
    }

    #[test]
    fn trait_setter_inserts_meta_when_absent() {
        let mut params = Params::default();
        assert_eq!(params.traceparent(), None);
        params.set_traceparent(TRACEPARENT);
        assert_eq!(params.traceparent(), Some(TRACEPARENT));
    }
}
