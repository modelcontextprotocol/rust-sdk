use std::ops::{Deref, DerefMut};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    ClientCapabilities, ClientNotification, ClientRequest, CustomNotification, CustomRequest,
    Extensions, Implementation, JsonObject, JsonRpcMessage, LoggingLevel, ProgressToken,
    ProtocolVersion, RequestId, ServerNotification, ServerRequest, TaskMetadata,
};

/// Access to the metadata carried by a message envelope's [`Extensions`].
///
/// The metadata type differs by message kind: requests carry a
/// [`RequestMetaObject`] and notifications carry a [`NotificationMetaObject`].
///
/// The envelope extensions are the canonical runtime location for `_meta`:
/// deserialization strips the wire `params._meta` into the extensions (typed
/// params `meta` fields stay empty), and the service loop moves it into
/// [`RequestContext::meta`] / [`NotificationContext::meta`] before dispatch.
/// Typed params `meta` fields are honored when serializing outgoing messages;
/// on key conflicts the extensions-level metadata wins.
///
/// [`RequestContext::meta`]: crate::service::RequestContext
/// [`NotificationContext::meta`]: crate::service::NotificationContext
pub trait GetMeta {
    /// The metadata type for this message kind.
    type Metadata: Default;
    fn get_meta_mut(&mut self) -> &mut Self::Metadata;
    fn get_meta(&self) -> &Self::Metadata;
}

pub trait GetExtensions {
    fn extensions(&self) -> &Extensions;
    fn extensions_mut(&mut self) -> &mut Extensions;
}

/// Trait for request params that contain the `_meta` field.
///
/// Per the MCP spec, all request params may have an optional `_meta`
/// field ([`RequestMetaObject`]) that can contain a `progressToken` for
/// tracking long-running operations.
pub trait RequestParamsMeta {
    /// Get a reference to the meta field
    fn meta(&self) -> Option<&RequestMetaObject>;
    /// Get a mutable reference to the meta field
    fn meta_mut(&mut self) -> &mut Option<RequestMetaObject>;
    /// Set the meta field
    fn set_meta(&mut self, meta: RequestMetaObject) {
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
                let mut meta = RequestMetaObject::new();
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
    fn meta_or_default(&mut self) -> &mut RequestMetaObject {
        self.meta_mut().get_or_insert_with(RequestMetaObject::new)
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
    type Metadata = NotificationMetaObject;
    fn get_meta_mut(&mut self) -> &mut NotificationMetaObject {
        self.extensions_mut().get_or_insert_default()
    }
    fn get_meta(&self) -> &NotificationMetaObject {
        self.extensions()
            .get::<NotificationMetaObject>()
            .unwrap_or(NotificationMetaObject::static_empty())
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
    type Metadata = RequestMetaObject;
    fn get_meta_mut(&mut self) -> &mut RequestMetaObject {
        self.extensions_mut().get_or_insert_default()
    }
    fn get_meta(&self) -> &RequestMetaObject {
        self.extensions()
            .get::<RequestMetaObject>()
            .unwrap_or(RequestMetaObject::static_empty())
    }
}

macro_rules! variant_extension {
    (
        $Enum: ident: $Metadata: ident {
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
            type Metadata = $Metadata;
            fn get_meta_mut(&mut self) -> &mut $Metadata {
                self.extensions_mut().get_or_insert_default()
            }
            fn get_meta(&self) -> &$Metadata {
                self.extensions().get::<$Metadata>().unwrap_or($Metadata::static_empty())
            }
        }
    };
}

variant_extension! {
    ClientRequest: RequestMetaObject {
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
    ServerRequest: RequestMetaObject {
        PingRequest
        CreateMessageRequest
        ListRootsRequest
        ElicitRequest
        CustomRequest
    }
}

variant_extension! {
    ClientNotification: NotificationMetaObject {
        CancelledNotification
        ProgressNotification
        InitializedNotification
        RootsListChangedNotification
        TaskStatusNotification
        CustomNotification
    }
}

variant_extension! {
    ServerNotification: NotificationMetaObject {
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

/// General-purpose `_meta` map (spec `MetaObject`).
///
/// This is the metadata shape used by results, content blocks, and catalog
/// descriptors (tools, prompts, resources, roots, ...). It preserves arbitrary
/// extension keys and offers helpers for the reserved W3C Trace Context keys
/// (SEP-414).
///
/// Request and notification `_meta` maps have additional reserved keys; see
/// [`RequestMetaObject`] and [`NotificationMetaObject`].
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
#[serde(transparent)]
#[expect(clippy::exhaustive_structs, reason = "intentionally exhaustive")]
pub struct MetaObject(pub JsonObject);

/// Deprecated alias for [`MetaObject`].
///
/// This is a re-export rather than a type alias so the `Meta(...)` tuple
/// constructor keeps working. Request and notification metadata now have
/// dedicated types; use [`RequestMetaObject`] or [`NotificationMetaObject`]
/// where those are expected.
#[deprecated(note = "Use MetaObject (or RequestMetaObject / NotificationMetaObject)")]
pub use self::MetaObject as Meta;

impl MetaObject {
    /// Reserved `_meta` key for the W3C Trace Context `traceparent` value (SEP-414).
    const TRACEPARENT_FIELD: &str = "traceparent";
    /// Reserved `_meta` key for the W3C Trace Context `tracestate` value (SEP-414).
    const TRACESTATE_FIELD: &str = "tracestate";
    /// Reserved `_meta` key for the W3C Baggage value (SEP-414).
    const BAGGAGE_FIELD: &str = "baggage";

    /// Create an empty metadata map.
    pub fn new() -> Self {
        Self(JsonObject::new())
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
    /// use rmcp::model::MetaObject;
    ///
    /// let mut meta = MetaObject::new();
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

    /// Insert every entry of `other`, overwriting existing keys on conflict.
    pub fn extend(&mut self, other: MetaObject) {
        self.0.extend(other.0);
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

impl Deref for MetaObject {
    type Target = JsonObject;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for MetaObject {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<JsonObject> for MetaObject {
    fn from(object: JsonObject) -> Self {
        Self(object)
    }
}

#[cfg(feature = "schemars")]
impl schemars::JsonSchema for MetaObject {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("MetaObject")
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "See [specification/draft/basic/index#general-fields] for notes on _meta usage.",
            "type": "object",
            "additionalProperties": true,
        })
    }
}

/// The `_meta` map carried by requests (spec `RequestMetaObject`).
///
/// In addition to arbitrary extension keys, requests reserve:
/// - `progressToken` for progress tracking
/// - `io.modelcontextprotocol/protocolVersion` (SEP-2575)
/// - `io.modelcontextprotocol/clientInfo` (SEP-2575)
/// - `io.modelcontextprotocol/clientCapabilities` (SEP-2575)
/// - `io.modelcontextprotocol/logLevel` (SEP-2575)
///
/// The 2026-07-28 draft schema marks the protocol-version, client-info, and
/// client-capabilities keys as required; earlier protocol versions do not know
/// them. All keys therefore stay optional at runtime and in the generated
/// (version-shared) JSON schema — use
/// [`RequestMetaObject::missing_required_keys`] to validate a request against
/// the negotiated protocol version.
///
/// This type dereferences to [`MetaObject`] (and transitively to the underlying
/// map), so general helpers such as the SEP-414 trace-context accessors remain
/// available.
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
#[serde(transparent)]
#[expect(clippy::exhaustive_structs, reason = "intentionally exhaustive")]
pub struct RequestMetaObject(pub MetaObject);

impl RequestMetaObject {
    const PROGRESS_TOKEN_FIELD: &str = "progressToken";
    const META_KEY_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
    const META_KEY_CLIENT_INFO: &str = "io.modelcontextprotocol/clientInfo";
    const META_KEY_CLIENT_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
    const META_KEY_LOG_LEVEL: &str = "io.modelcontextprotocol/logLevel";

    /// Request `_meta` keys the 2026-07-28 draft schema marks as required.
    pub const DRAFT_REQUIRED_KEYS: [&str; 3] = [
        Self::META_KEY_PROTOCOL_VERSION,
        Self::META_KEY_CLIENT_INFO,
        Self::META_KEY_CLIENT_CAPABILITIES,
    ];

    /// Create an empty request metadata map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new request meta with a progress token set
    pub fn with_progress_token(token: ProgressToken) -> Self {
        let mut meta = Self::new();
        meta.set_progress_token(token);
        meta
    }

    pub(crate) fn static_empty() -> &'static Self {
        static EMPTY: std::sync::OnceLock<RequestMetaObject> = std::sync::OnceLock::new();
        EMPTY.get_or_init(Default::default)
    }

    /// Get the progress token carried in `_meta`, if present and valid.
    pub fn get_progress_token(&self) -> Option<ProgressToken> {
        self.0.decode_value(Self::PROGRESS_TOKEN_FIELD)
    }

    /// Set the progress token carried in `_meta`.
    pub fn set_progress_token(&mut self, token: ProgressToken) {
        self.0.insert_serialized(Self::PROGRESS_TOKEN_FIELD, token);
    }

    /// Get the MCP protocol version carried in `_meta`, if present and valid.
    pub fn protocol_version(&self) -> Option<ProtocolVersion> {
        self.0.decode_value(Self::META_KEY_PROTOCOL_VERSION)
    }

    /// Set the MCP protocol version carried in `_meta`.
    pub fn set_protocol_version(&mut self, protocol_version: ProtocolVersion) {
        self.0.0.insert(
            Self::META_KEY_PROTOCOL_VERSION.to_string(),
            Value::String(protocol_version.to_string()),
        );
    }

    /// Get the client implementation identity carried in `_meta`, if present and valid.
    pub fn client_info(&self) -> Option<Implementation> {
        self.0.decode_value(Self::META_KEY_CLIENT_INFO)
    }

    /// Set the client implementation identity carried in `_meta`.
    pub fn set_client_info(&mut self, client_info: Implementation) {
        self.0
            .insert_serialized(Self::META_KEY_CLIENT_INFO, client_info);
    }

    /// Get the client capabilities carried in `_meta`, if present and valid.
    pub fn client_capabilities(&self) -> Option<ClientCapabilities> {
        self.0.decode_value(Self::META_KEY_CLIENT_CAPABILITIES)
    }

    /// Set the client capabilities carried in `_meta`.
    pub fn set_client_capabilities(&mut self, client_capabilities: ClientCapabilities) {
        self.0
            .insert_serialized(Self::META_KEY_CLIENT_CAPABILITIES, client_capabilities);
    }

    /// Get the requested per-request log level carried in `_meta`, if present and valid.
    pub fn log_level(&self) -> Option<LoggingLevel> {
        self.0.decode_value(Self::META_KEY_LOG_LEVEL)
    }

    /// Set the requested per-request log level carried in `_meta`.
    pub fn set_log_level(&mut self, log_level: LoggingLevel) {
        self.0
            .insert_serialized(Self::META_KEY_LOG_LEVEL, log_level);
    }

    /// Return the [`Self::DRAFT_REQUIRED_KEYS`] whose values are absent or
    /// invalid in this map, if `protocol_version` requires them.
    ///
    /// A key counts as missing when it is not present *or* when its value does
    /// not decode into the expected type (e.g. a numeric `protocolVersion` or
    /// a string `clientInfo`), matching what the typed accessors return.
    ///
    /// Protocol versions before 2026-07-28 have no required request metadata,
    /// so this always returns an empty list for them.
    ///
    /// # Examples
    ///
    /// ```
    /// use rmcp::model::{ProtocolVersion, RequestMetaObject};
    ///
    /// let meta = RequestMetaObject::new();
    /// // Older protocols have no required request metadata.
    /// assert!(
    ///     meta.missing_required_keys(&ProtocolVersion::V_2025_11_25)
    ///         .is_empty()
    /// );
    /// // The 2026-07-28 draft requires the SEP-2575 keys.
    /// assert_eq!(
    ///     meta.missing_required_keys(&ProtocolVersion::V_2026_07_28),
    ///     RequestMetaObject::DRAFT_REQUIRED_KEYS.to_vec(),
    /// );
    /// ```
    pub fn missing_required_keys(&self, protocol_version: &ProtocolVersion) -> Vec<&'static str> {
        if protocol_version.as_str() < ProtocolVersion::V_2026_07_28.as_str() {
            return Vec::new();
        }
        let mut missing = Vec::new();
        if self.protocol_version().is_none() {
            missing.push(Self::META_KEY_PROTOCOL_VERSION);
        }
        if self.client_info().is_none() {
            missing.push(Self::META_KEY_CLIENT_INFO);
        }
        if self.client_capabilities().is_none() {
            missing.push(Self::META_KEY_CLIENT_CAPABILITIES);
        }
        missing
    }

    /// Insert every entry of `other`, overwriting existing keys on conflict.
    pub fn extend(&mut self, other: RequestMetaObject) {
        self.0.extend(other.0);
    }
}

impl Deref for RequestMetaObject {
    type Target = MetaObject;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for RequestMetaObject {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<MetaObject> for RequestMetaObject {
    fn from(meta: MetaObject) -> Self {
        Self(meta)
    }
}

impl From<JsonObject> for RequestMetaObject {
    fn from(object: JsonObject) -> Self {
        Self(MetaObject(object))
    }
}

#[cfg(feature = "schemars")]
impl schemars::JsonSchema for RequestMetaObject {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("RequestMetaObject")
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let progress_token = generator.subschema_for::<ProgressToken>();
        let client_info = generator.subschema_for::<Implementation>();
        let client_capabilities = generator.subschema_for::<ClientCapabilities>();
        let log_level = generator.subschema_for::<LoggingLevel>();
        // rmcp generates one schema shared by every supported protocol
        // version, so the keys the 2026-07-28 draft marks as required are left
        // optional here: a 2025-11-25 request whose `_meta` only carries
        // `progressToken` is valid. Draft-strict validation is available at
        // runtime via [`RequestMetaObject::missing_required_keys`].
        schemars::json_schema!({
            "description": "Metadata reserved by MCP on requests. Extension keys are also allowed.",
            "type": "object",
            "properties": {
                "progressToken": progress_token,
                "io.modelcontextprotocol/protocolVersion": {
                    "type": "string",
                },
                "io.modelcontextprotocol/clientInfo": client_info,
                "io.modelcontextprotocol/clientCapabilities": client_capabilities,
                "io.modelcontextprotocol/logLevel": log_level,
            },
            "additionalProperties": true,
        })
    }
}

/// The `_meta` map carried by notifications (spec `NotificationMetaObject`).
///
/// In addition to arbitrary extension keys, notifications reserve
/// `io.modelcontextprotocol/subscriptionId` to correlate a notification with a
/// prior subscription request.
///
/// This type dereferences to [`MetaObject`] (and transitively to the underlying
/// map), so general helpers such as the SEP-414 trace-context accessors remain
/// available.
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
#[serde(transparent)]
#[expect(clippy::exhaustive_structs, reason = "intentionally exhaustive")]
pub struct NotificationMetaObject(pub MetaObject);

impl NotificationMetaObject {
    const META_KEY_SUBSCRIPTION_ID: &str = "io.modelcontextprotocol/subscriptionId";

    /// Create an empty notification metadata map.
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn static_empty() -> &'static Self {
        static EMPTY: std::sync::OnceLock<NotificationMetaObject> = std::sync::OnceLock::new();
        EMPTY.get_or_init(Default::default)
    }

    /// Get the subscription id carried in `_meta`, if present and valid.
    ///
    /// # Examples
    ///
    /// ```
    /// use rmcp::model::{NotificationMetaObject, RequestId};
    ///
    /// let mut meta = NotificationMetaObject::new();
    /// assert_eq!(meta.subscription_id(), None);
    /// meta.set_subscription_id(RequestId::Number(7));
    /// assert_eq!(meta.subscription_id(), Some(RequestId::Number(7)));
    /// ```
    pub fn subscription_id(&self) -> Option<RequestId> {
        self.0.decode_value(Self::META_KEY_SUBSCRIPTION_ID)
    }

    /// Set the subscription id carried in `_meta`.
    pub fn set_subscription_id(&mut self, subscription_id: RequestId) {
        self.0
            .insert_serialized(Self::META_KEY_SUBSCRIPTION_ID, subscription_id);
    }

    /// Insert every entry of `other`, overwriting existing keys on conflict.
    pub fn extend(&mut self, other: NotificationMetaObject) {
        self.0.extend(other.0);
    }
}

impl Deref for NotificationMetaObject {
    type Target = MetaObject;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for NotificationMetaObject {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<MetaObject> for NotificationMetaObject {
    fn from(meta: MetaObject) -> Self {
        Self(meta)
    }
}

impl From<JsonObject> for NotificationMetaObject {
    fn from(object: JsonObject) -> Self {
        Self(MetaObject(object))
    }
}

#[cfg(feature = "schemars")]
impl schemars::JsonSchema for NotificationMetaObject {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("NotificationMetaObject")
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let subscription_id = generator.subschema_for::<RequestId>();
        schemars::json_schema!({
            "description": "Metadata reserved by MCP on notifications. Extension keys are also allowed.",
            "type": "object",
            "properties": {
                "io.modelcontextprotocol/subscriptionId": subscription_id,
            },
            "additionalProperties": true,
        })
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
    use crate::model::NumberOrString;

    #[derive(Default)]
    struct Params {
        meta: Option<RequestMetaObject>,
    }

    impl RequestParamsMeta for Params {
        fn meta(&self) -> Option<&RequestMetaObject> {
            self.meta.as_ref()
        }
        fn meta_mut(&mut self) -> &mut Option<RequestMetaObject> {
            &mut self.meta
        }
    }

    const TRACEPARENT: &str = "00-0af7651916cd43dd8448eb211c80319c-00f067aa0ba902b7-01";

    #[test]
    fn trace_context_round_trip() {
        let mut meta = MetaObject::new();
        meta.set_traceparent(TRACEPARENT);
        meta.set_tracestate("vendor1=value1,vendor2=value2");
        meta.set_baggage("userId=alice,region=us-east-1");
        assert_eq!(meta.get_traceparent(), Some(TRACEPARENT));
        assert_eq!(meta.get_tracestate(), Some("vendor1=value1,vendor2=value2"));
        assert_eq!(meta.get_baggage(), Some("userId=alice,region=us-east-1"));
    }

    #[test]
    fn absent_field_is_none() {
        let meta = MetaObject::new();
        assert_eq!(meta.get_traceparent(), None);
        assert_eq!(meta.get_tracestate(), None);
        assert_eq!(meta.get_baggage(), None);
    }

    #[test]
    fn non_string_value_is_none() {
        let mut meta = MetaObject::new();
        meta.0
            .insert(MetaObject::TRACEPARENT_FIELD.to_string(), Value::from(42));
        assert_eq!(meta.get_traceparent(), None);
    }

    #[test]
    fn trait_setter_inserts_meta_when_absent() {
        let mut params = Params::default();
        assert_eq!(params.traceparent(), None);
        params.set_traceparent(TRACEPARENT);
        assert_eq!(params.traceparent(), Some(TRACEPARENT));
    }

    #[test]
    fn request_meta_derefs_to_general_helpers() {
        let mut meta = RequestMetaObject::new();
        meta.set_traceparent(TRACEPARENT);
        meta.set_progress_token(ProgressToken(NumberOrString::Number(7)));
        assert_eq!(meta.get_traceparent(), Some(TRACEPARENT));
        assert_eq!(
            meta.get_progress_token(),
            Some(ProgressToken(NumberOrString::Number(7)))
        );
    }

    mod subscription_id {
        use super::*;

        #[test]
        fn returns_none_when_absent() {
            let meta = NotificationMetaObject::new();
            assert_eq!(meta.subscription_id(), None);
        }

        #[test]
        fn round_trips_number_id() {
            let mut meta = NotificationMetaObject::new();
            meta.set_subscription_id(RequestId::Number(42));
            assert_eq!(meta.subscription_id(), Some(RequestId::Number(42)));
        }

        #[test]
        fn round_trips_string_id() {
            let mut meta = NotificationMetaObject::new();
            meta.set_subscription_id(RequestId::String("sub-1".into()));
            assert_eq!(
                meta.subscription_id(),
                Some(RequestId::String("sub-1".into()))
            );
        }
    }

    mod missing_required_keys {
        use super::*;

        #[test]
        fn is_empty_for_pre_draft_protocols() {
            let meta = RequestMetaObject::new();
            assert!(
                meta.missing_required_keys(&ProtocolVersion::V_2025_11_25)
                    .is_empty()
            );
        }

        #[test]
        fn lists_all_draft_keys_for_empty_meta() {
            let meta = RequestMetaObject::new();
            assert_eq!(
                meta.missing_required_keys(&ProtocolVersion::V_2026_07_28),
                RequestMetaObject::DRAFT_REQUIRED_KEYS.to_vec()
            );
        }

        #[test]
        fn treats_malformed_values_as_missing() {
            let meta: RequestMetaObject = serde_json::from_value(serde_json::json!({
                "io.modelcontextprotocol/protocolVersion": 123,
                "io.modelcontextprotocol/clientInfo": "not an implementation",
                "io.modelcontextprotocol/clientCapabilities": null,
            }))
            .unwrap();
            assert_eq!(
                meta.missing_required_keys(&ProtocolVersion::V_2026_07_28),
                RequestMetaObject::DRAFT_REQUIRED_KEYS.to_vec()
            );
        }

        #[test]
        fn is_empty_when_draft_keys_are_present() {
            let mut meta = RequestMetaObject::new();
            meta.set_protocol_version(ProtocolVersion::V_2026_07_28);
            meta.set_client_info(Implementation::from_build_env());
            meta.set_client_capabilities(ClientCapabilities::default());
            assert!(
                meta.missing_required_keys(&ProtocolVersion::V_2026_07_28)
                    .is_empty()
            );
        }
    }
}
