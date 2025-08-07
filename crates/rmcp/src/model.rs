use std::{borrow::Cow, sync::Arc};
mod annotated;
mod capabilities;
mod content;
mod extension;
mod meta;
mod prompt;
mod resource;
mod serde_impl;
mod tool;
pub use annotated::*;
pub use capabilities::*;
pub use content::*;
pub use extension::*;
pub use meta::*;
pub use prompt::*;
pub use resource::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub use tool::*;

/// A JSON object type alias for convenient handling of JSON data.
///
/// You can use [`crate::object!`] or [`crate::model::object`] to create a json object quickly.
/// This is commonly used for storing arbitrary JSON data in MCP messages.
pub type JsonObject<F = Value> = serde_json::Map<String, F>;

/// unwrap the JsonObject under [`serde_json::Value`]
///
/// # Panic
/// This will panic when the value is not a object in debug mode.
pub fn object(value: serde_json::Value) -> JsonObject {
    debug_assert!(value.is_object());
    match value {
        serde_json::Value::Object(map) => map,
        _ => JsonObject::default(),
    }
}

/// Use this macro just like [`serde_json::json!`]
#[cfg(feature = "macros")]
#[cfg_attr(docsrs, doc(cfg(feature = "macros")))]
#[macro_export]
macro_rules! object {
    ({$($tt:tt)*}) => {
        $crate::model::object(serde_json::json! {
            {$($tt)*}
        })
    };
}

/// This is commonly used for representing empty objects in MCP messages.
///
/// without returning any specific data.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Copy, Eq)]
#[cfg_attr(feature = "server", derive(schemars::JsonSchema))]
pub struct EmptyObject {}

pub trait ConstString: Default {
    const VALUE: &str;
}
#[macro_export]
macro_rules! const_string {
    ($name:ident = $value:literal) => {
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
        pub struct $name;

        impl ConstString for $name {
            const VALUE: &str = $value;
        }

        impl serde::Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                $value.serialize(serializer)
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<$name, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let s: String = serde::Deserialize::deserialize(deserializer)?;
                if s == $value {
                    Ok($name)
                } else {
                    Err(serde::de::Error::custom(format!(concat!(
                        "expect const string value \"",
                        $value,
                        "\""
                    ))))
                }
            }
        }

        #[cfg(feature = "schemars")]
        impl schemars::JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> {
                Cow::Borrowed(stringify!($name))
            }

            fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
                use serde_json::{Map, json};

                let mut schema_map = Map::new();
                schema_map.insert("type".to_string(), json!("string"));
                schema_map.insert("format".to_string(), json!("const"));
                schema_map.insert("const".to_string(), json!($value));

                schemars::Schema::from(schema_map)
            }
        }
    };
}

const_string!(JsonRpcVersion2_0 = "2.0");

// =============================================================================
// CORE PROTOCOL TYPES
// =============================================================================

/// Represents the MCP protocol version used for communication.
///
/// This ensures compatibility between clients and servers by specifying
/// which version of the Model Context Protocol is being used.
#[derive(Debug, Clone, Eq, PartialEq, Hash, PartialOrd)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ProtocolVersion(Cow<'static, str>);

impl Default for ProtocolVersion {
    fn default() -> Self {
        Self::LATEST
    }
}

impl std::fmt::Display for ProtocolVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl ProtocolVersion {
    pub const V_2025_03_26: Self = Self(Cow::Borrowed("2025-03-26"));
    pub const V_2024_11_05: Self = Self(Cow::Borrowed("2024-11-05"));
    pub const LATEST: Self = Self::V_2025_03_26;
}

impl Serialize for ProtocolVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ProtocolVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s: String = Deserialize::deserialize(deserializer)?;
        #[allow(clippy::single_match)]
        match s.as_str() {
            "2024-11-05" => return Ok(ProtocolVersion::V_2024_11_05),
            "2025-03-26" => return Ok(ProtocolVersion::V_2025_03_26),
            _ => {}
        }
        Ok(ProtocolVersion(Cow::Owned(s)))
    }
}

/// A flexible identifier type that can be either a number or a string.
///
/// This is commonly used for request IDs and other identifiers in JSON-RPC
/// where the specification allows both numeric and string values.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum NumberOrString {
    /// A numeric identifier
    Number(u32),
    /// A string identifier  
    String(Arc<str>),
}

impl NumberOrString {
    pub fn into_json_value(self) -> Value {
        match self {
            NumberOrString::Number(n) => Value::Number(serde_json::Number::from(n)),
            NumberOrString::String(s) => Value::String(s.to_string()),
        }
    }
}

impl std::fmt::Display for NumberOrString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NumberOrString::Number(n) => n.fmt(f),
            NumberOrString::String(s) => s.fmt(f),
        }
    }
}

impl Serialize for NumberOrString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            NumberOrString::Number(n) => n.serialize(serializer),
            NumberOrString::String(s) => s.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for NumberOrString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value: Value = Deserialize::deserialize(deserializer)?;
        match value {
            Value::Number(n) => Ok(NumberOrString::Number(
                n.as_u64()
                    .ok_or(serde::de::Error::custom("Expect an integer"))? as u32,
            )),
            Value::String(s) => Ok(NumberOrString::String(s.into())),
            _ => Err(serde::de::Error::custom("Expect number or string")),
        }
    }
}

#[cfg(feature = "schemars")]
impl schemars::JsonSchema for NumberOrString {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("NumberOrString")
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        use serde_json::{Map, json};

        let mut number_schema = Map::new();
        number_schema.insert("type".to_string(), json!("number"));

        let mut string_schema = Map::new();
        string_schema.insert("type".to_string(), json!("string"));

        let mut schema_map = Map::new();
        schema_map.insert("oneOf".to_string(), json!([number_schema, string_schema]));

        schemars::Schema::from(schema_map)
    }
}

/// Type alias for request identifiers used in JSON-RPC communication.
pub type RequestId = NumberOrString;

/// A token used to track the progress of long-running operations.
///
/// Progress tokens allow clients and servers to associate progress notifications
/// with specific requests, enabling real-time updates on operation status.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Hash, Eq)]
#[serde(transparent)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ProgressToken(pub NumberOrString);

// =============================================================================
// JSON-RPC MESSAGE STRUCTURES
// =============================================================================

/// Represents a JSON-RPC request with method, parameters, and extensions.
///
/// This is the core structure for all MCP requests, containing:
/// - `method`: The name of the method being called
/// - `params`: The parameters for the method
/// - `extensions`: Additional context data (similar to HTTP headers)
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct Request<M = String, P = JsonObject> {
    pub method: M,
    pub params: P,
    /// extensions will carry anything possible in the context, including [`Meta`]
    ///
    /// this is similar with the Extensions in `http` crate
    #[cfg_attr(feature = "schemars", schemars(skip))]
    pub extensions: Extensions,
}

impl<M: Default, P> Request<M, P> {
    pub fn new(params: P) -> Self {
        Self {
            method: Default::default(),
            params,
            extensions: Extensions::default(),
        }
    }
}

impl<M, P> GetExtensions for Request<M, P> {
    fn extensions(&self) -> &Extensions {
        &self.extensions
    }
    fn extensions_mut(&mut self) -> &mut Extensions {
        &mut self.extensions
    }
}

#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct RequestOptionalParam<M = String, P = JsonObject> {
    pub method: M,
    // #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<P>,
    /// extensions will carry anything possible in the context, including [`Meta`]
    ///
    /// this is similar with the Extensions in `http` crate
    #[cfg_attr(feature = "schemars", schemars(skip))]
    pub extensions: Extensions,
}

impl<M: Default, P> RequestOptionalParam<M, P> {
    pub fn with_param(params: P) -> Self {
        Self {
            method: Default::default(),
            params: Some(params),
            extensions: Extensions::default(),
        }
    }
}

#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct RequestNoParam<M = String> {
    pub method: M,
    /// extensions will carry anything possible in the context, including [`Meta`]
    ///
    /// this is similar with the Extensions in `http` crate
    #[cfg_attr(feature = "schemars", schemars(skip))]
    pub extensions: Extensions,
}

impl<M> GetExtensions for RequestNoParam<M> {
    fn extensions(&self) -> &Extensions {
        &self.extensions
    }
    fn extensions_mut(&mut self) -> &mut Extensions {
        &mut self.extensions
    }
}
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct Notification<M = String, P = JsonObject> {
    pub method: M,
    pub params: P,
    /// extensions will carry anything possible in the context, including [`Meta`]
    ///
    /// this is similar with the Extensions in `http` crate
    #[cfg_attr(feature = "schemars", schemars(skip))]
    pub extensions: Extensions,
}

impl<M: Default, P> Notification<M, P> {
    pub fn new(params: P) -> Self {
        Self {
            method: Default::default(),
            params,
            extensions: Extensions::default(),
        }
    }
}

#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct NotificationNoParam<M = String> {
    pub method: M,
    /// extensions will carry anything possible in the context, including [`Meta`]
    ///
    /// this is similar with the Extensions in `http` crate
    #[cfg_attr(feature = "schemars", schemars(skip))]
    pub extensions: Extensions,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct JsonRpcRequest<R = Request> {
    pub jsonrpc: JsonRpcVersion2_0,
    pub id: RequestId,
    #[serde(flatten)]
    pub request: R,
}

type DefaultResponse = JsonObject;
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct JsonRpcResponse<R = JsonObject> {
    pub jsonrpc: JsonRpcVersion2_0,
    pub id: RequestId,
    pub result: R,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct JsonRpcError {
    pub jsonrpc: JsonRpcVersion2_0,
    pub id: RequestId,
    pub error: ErrorData,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct JsonRpcNotification<N = Notification> {
    pub jsonrpc: JsonRpcVersion2_0,
    #[serde(flatten)]
    pub notification: N,
}

/// Standard JSON-RPC error codes used throughout the MCP protocol.
///
/// These codes follow the JSON-RPC 2.0 specification and provide
/// standardized error reporting across all MCP implementations.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ErrorCode(pub i32);

impl ErrorCode {
    pub const RESOURCE_NOT_FOUND: Self = Self(-32002);
    pub const INVALID_REQUEST: Self = Self(-32600);
    pub const METHOD_NOT_FOUND: Self = Self(-32601);
    pub const INVALID_PARAMS: Self = Self(-32602);
    pub const INTERNAL_ERROR: Self = Self(-32603);
    pub const PARSE_ERROR: Self = Self(-32700);
}

/// Error information for JSON-RPC error responses.
///
/// This structure follows the JSON-RPC 2.0 specification for error reporting,
/// providing a standardized way to communicate errors between clients and servers.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ErrorData {
    /// The error type that occurred (using standard JSON-RPC error codes)
    pub code: ErrorCode,

    /// A short description of the error. The message SHOULD be limited to a concise single sentence.
    pub message: Cow<'static, str>,

    /// Additional information about the error. The value of this member is defined by the
    /// sender (e.g. detailed error information, nested errors etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl ErrorData {
    pub fn new(
        code: ErrorCode,
        message: impl Into<Cow<'static, str>>,
        data: Option<Value>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            data,
        }
    }
    pub fn resource_not_found(message: impl Into<Cow<'static, str>>, data: Option<Value>) -> Self {
        Self::new(ErrorCode::RESOURCE_NOT_FOUND, message, data)
    }
    pub fn parse_error(message: impl Into<Cow<'static, str>>, data: Option<Value>) -> Self {
        Self::new(ErrorCode::PARSE_ERROR, message, data)
    }
    pub fn invalid_request(message: impl Into<Cow<'static, str>>, data: Option<Value>) -> Self {
        Self::new(ErrorCode::INVALID_REQUEST, message, data)
    }
    pub fn method_not_found<M: ConstString>() -> Self {
        Self::new(ErrorCode::METHOD_NOT_FOUND, M::VALUE, None)
    }
    pub fn invalid_params(message: impl Into<Cow<'static, str>>, data: Option<Value>) -> Self {
        Self::new(ErrorCode::INVALID_PARAMS, message, data)
    }
    pub fn internal_error(message: impl Into<Cow<'static, str>>, data: Option<Value>) -> Self {
        Self::new(ErrorCode::INTERNAL_ERROR, message, data)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum JsonRpcBatchRequestItem<Req, Not> {
    Request(JsonRpcRequest<Req>),
    Notification(JsonRpcNotification<Not>),
}

impl<Req, Not> JsonRpcBatchRequestItem<Req, Not> {
    pub fn into_non_batch_message<Resp>(self) -> JsonRpcMessage<Req, Resp, Not> {
        match self {
            JsonRpcBatchRequestItem::Request(r) => JsonRpcMessage::Request(r),
            JsonRpcBatchRequestItem::Notification(n) => JsonRpcMessage::Notification(n),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum JsonRpcBatchResponseItem<Resp> {
    Response(JsonRpcResponse<Resp>),
    Error(JsonRpcError),
}

impl<Resp> JsonRpcBatchResponseItem<Resp> {
    pub fn into_non_batch_message<Req, Not>(self) -> JsonRpcMessage<Req, Resp, Not> {
        match self {
            JsonRpcBatchResponseItem::Response(r) => JsonRpcMessage::Response(r),
            JsonRpcBatchResponseItem::Error(e) => JsonRpcMessage::Error(e),
        }
    }
}

/// Represents any JSON-RPC message that can be sent or received.
///
/// This enum covers all possible message types in the JSON-RPC protocol:
/// individual requests/responses, notifications, batch operations, and errors.
/// It serves as the top-level message container for MCP communication.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum JsonRpcMessage<Req = Request, Resp = DefaultResponse, Noti = Notification> {
    /// A single request expecting a response
    Request(JsonRpcRequest<Req>),
    /// A response to a previous request
    Response(JsonRpcResponse<Resp>),
    /// A one-way notification (no response expected)
    Notification(JsonRpcNotification<Noti>),
    /// Multiple requests sent together
    BatchRequest(Vec<JsonRpcBatchRequestItem<Req, Noti>>),
    /// Multiple responses sent together
    BatchResponse(Vec<JsonRpcBatchResponseItem<Resp>>),
    /// An error response
    Error(JsonRpcError),
}

impl<Req, Resp, Not> JsonRpcMessage<Req, Resp, Not> {
    #[inline]
    pub const fn request(request: Req, id: RequestId) -> Self {
        JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JsonRpcVersion2_0,
            id,
            request,
        })
    }
    #[inline]
    pub const fn response(response: Resp, id: RequestId) -> Self {
        JsonRpcMessage::Response(JsonRpcResponse {
            jsonrpc: JsonRpcVersion2_0,
            id,
            result: response,
        })
    }
    #[inline]
    pub const fn error(error: ErrorData, id: RequestId) -> Self {
        JsonRpcMessage::Error(JsonRpcError {
            jsonrpc: JsonRpcVersion2_0,
            id,
            error,
        })
    }
    #[inline]
    pub const fn notification(notification: Not) -> Self {
        JsonRpcMessage::Notification(JsonRpcNotification {
            jsonrpc: JsonRpcVersion2_0,
            notification,
        })
    }
    pub fn into_request(self) -> Option<(Req, RequestId)> {
        match self {
            JsonRpcMessage::Request(r) => Some((r.request, r.id)),
            _ => None,
        }
    }
    pub fn into_response(self) -> Option<(Resp, RequestId)> {
        match self {
            JsonRpcMessage::Response(r) => Some((r.result, r.id)),
            _ => None,
        }
    }
    pub fn into_notification(self) -> Option<Not> {
        match self {
            JsonRpcMessage::Notification(n) => Some(n.notification),
            _ => None,
        }
    }
    pub fn into_error(self) -> Option<(ErrorData, RequestId)> {
        match self {
            JsonRpcMessage::Error(e) => Some((e.error, e.id)),
            _ => None,
        }
    }
    pub fn into_result(self) -> Option<(Result<Resp, ErrorData>, RequestId)> {
        match self {
            JsonRpcMessage::Response(r) => Some((Ok(r.result), r.id)),
            JsonRpcMessage::Error(e) => Some((Err(e.error), e.id)),

            _ => None,
        }
    }
}

// =============================================================================
// INITIALIZATION AND CONNECTION SETUP
// =============================================================================

/// # Empty result
/// A response that indicates success but carries no data.
pub type EmptyResult = EmptyObject;

impl From<()> for EmptyResult {
    fn from(_value: ()) -> Self {
        EmptyResult {}
    }
}

impl From<EmptyResult> for () {
    fn from(_value: EmptyResult) {}
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct CancelledNotificationParam {
    pub request_id: RequestId,
    pub reason: Option<String>,
}

const_string!(CancelledNotificationMethod = "notifications/cancelled");

/// # Cancellation
/// This notification can be sent by either side to indicate that it is cancelling a previously-issued request.
///
/// The request SHOULD still be in-flight, but due to communication latency, it is always possible that this notification MAY arrive after the request has already finished.
///
/// This notification indicates that the result will be unused, so any associated processing SHOULD cease.
///
/// A client MUST NOT attempt to cancel its `initialize` request.
pub type CancelledNotification =
    Notification<CancelledNotificationMethod, CancelledNotificationParam>;

const_string!(InitializeResultMethod = "initialize");
/// # Initialization
/// This request is sent from the client to the server when it first connects, asking it to begin initialization.
pub type InitializeRequest = Request<InitializeResultMethod, InitializeRequestParam>;

const_string!(InitializedNotificationMethod = "notifications/initialized");
/// This notification is sent from the client to the server after initialization has finished.
pub type InitializedNotification = NotificationNoParam<InitializedNotificationMethod>;

/// Parameters sent by a client when initializing a connection to an MCP server.
///
/// This contains the client's protocol version, capabilities, and implementation
/// information, allowing the server to understand what the client supports.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct InitializeRequestParam {
    /// The MCP protocol version this client supports
    pub protocol_version: ProtocolVersion,
    /// The capabilities this client supports (sampling, roots, etc.)
    pub capabilities: ClientCapabilities,
    /// Information about the client implementation
    pub client_info: Implementation,
}

/// The server's response to an initialization request.
///
/// Contains the server's protocol version, capabilities, and implementation
/// information, along with optional instructions for the client.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct InitializeResult {
    /// The MCP protocol version this server supports
    pub protocol_version: ProtocolVersion,
    /// The capabilities this server provides (tools, resources, prompts, etc.)
    pub capabilities: ServerCapabilities,
    /// Information about the server implementation
    pub server_info: Implementation,
    /// Optional human-readable instructions about using this server
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

pub type ServerInfo = InitializeResult;
pub type ClientInfo = InitializeRequestParam;

impl Default for ServerInfo {
    fn default() -> Self {
        ServerInfo {
            protocol_version: ProtocolVersion::default(),
            capabilities: ServerCapabilities::default(),
            server_info: Implementation::from_build_env(),
            instructions: None,
        }
    }
}

impl Default for ClientInfo {
    fn default() -> Self {
        ClientInfo {
            protocol_version: ProtocolVersion::default(),
            capabilities: ClientCapabilities::default(),
            client_info: Implementation::from_build_env(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct Implementation {
    pub name: String,
    pub version: String,
}

impl Default for Implementation {
    fn default() -> Self {
        Self::from_build_env()
    }
}

impl Implementation {
    pub fn from_build_env() -> Self {
        Implementation {
            name: env!("CARGO_CRATE_NAME").to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct PaginatedRequestParam {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}
// =============================================================================
// PROGRESS AND PAGINATION
// =============================================================================

const_string!(PingRequestMethod = "ping");
pub type PingRequest = RequestNoParam<PingRequestMethod>;

const_string!(ProgressNotificationMethod = "notifications/progress");
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ProgressNotificationParam {
    pub progress_token: ProgressToken,
    /// The progress thus far. This should increase every time progress is made, even if the total is unknown.
    pub progress: f64,
    /// Total number of items to process (or total progress required), if known
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<f64>,
    /// An optional message describing the current progress.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

pub type ProgressNotification = Notification<ProgressNotificationMethod, ProgressNotificationParam>;

pub type Cursor = String;

macro_rules! paginated_result {
    ($t:ident {
        $i_item: ident: $t_item: ty
    }) => {
        #[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
        #[serde(rename_all = "camelCase")]
        #[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
        pub struct $t {
            #[serde(skip_serializing_if = "Option::is_none")]
            pub next_cursor: Option<Cursor>,
            pub $i_item: $t_item,
        }

        impl $t {
            pub fn with_all_items(
                items: $t_item,
            ) -> Self {
                Self {
                    next_cursor: None,
                    $i_item: items,
                }
            }
        }
    };
}

// =============================================================================
// RESOURCE MANAGEMENT
// =============================================================================

const_string!(ListResourcesRequestMethod = "resources/list");
/// Request to list all available resources from a server
pub type ListResourcesRequest =
    RequestOptionalParam<ListResourcesRequestMethod, PaginatedRequestParam>;

paginated_result!(ListResourcesResult {
    resources: Vec<Resource>
});

const_string!(ListResourceTemplatesRequestMethod = "resources/templates/list");
/// Request to list all available resource templates from a server
pub type ListResourceTemplatesRequest =
    RequestOptionalParam<ListResourceTemplatesRequestMethod, PaginatedRequestParam>;

paginated_result!(ListResourceTemplatesResult {
    resource_templates: Vec<ResourceTemplate>
});

const_string!(ReadResourceRequestMethod = "resources/read");
/// Parameters for reading a specific resource
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ReadResourceRequestParam {
    /// The URI of the resource to read
    pub uri: String,
}

/// Result containing the contents of a read resource
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ReadResourceResult {
    /// The actual content of the resource
    pub contents: Vec<ResourceContents>,
}

/// Request to read a specific resource
pub type ReadResourceRequest = Request<ReadResourceRequestMethod, ReadResourceRequestParam>;

const_string!(ResourceListChangedNotificationMethod = "notifications/resources/list_changed");
/// Notification sent when the list of available resources changes
pub type ResourceListChangedNotification =
    NotificationNoParam<ResourceListChangedNotificationMethod>;

const_string!(SubscribeRequestMethod = "resources/subscribe");
/// Parameters for subscribing to resource updates
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct SubscribeRequestParam {
    /// The URI of the resource to subscribe to
    pub uri: String,
}
/// Request to subscribe to resource updates
pub type SubscribeRequest = Request<SubscribeRequestMethod, SubscribeRequestParam>;

const_string!(UnsubscribeRequestMethod = "resources/unsubscribe");
/// Parameters for unsubscribing from resource updates
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct UnsubscribeRequestParam {
    /// The URI of the resource to unsubscribe from
    pub uri: String,
}
/// Request to unsubscribe from resource updates
pub type UnsubscribeRequest = Request<UnsubscribeRequestMethod, UnsubscribeRequestParam>;

const_string!(ResourceUpdatedNotificationMethod = "notifications/resources/updated");
/// Parameters for a resource update notification
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ResourceUpdatedNotificationParam {
    /// The URI of the resource that was updated
    pub uri: String,
}
/// Notification sent when a subscribed resource is updated
pub type ResourceUpdatedNotification =
    Notification<ResourceUpdatedNotificationMethod, ResourceUpdatedNotificationParam>;

// =============================================================================
// PROMPT MANAGEMENT
// =============================================================================

const_string!(ListPromptsRequestMethod = "prompts/list");
/// Request to list all available prompts from a server
pub type ListPromptsRequest = RequestOptionalParam<ListPromptsRequestMethod, PaginatedRequestParam>;

paginated_result!(ListPromptsResult {
    prompts: Vec<Prompt>
});

const_string!(GetPromptRequestMethod = "prompts/get");
/// Parameters for retrieving a specific prompt
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct GetPromptRequestParam {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<JsonObject>,
}
/// Request to get a specific prompt
pub type GetPromptRequest = Request<GetPromptRequestMethod, GetPromptRequestParam>;

const_string!(PromptListChangedNotificationMethod = "notifications/prompts/list_changed");
/// Notification sent when the list of available prompts changes
pub type PromptListChangedNotification = NotificationNoParam<PromptListChangedNotificationMethod>;

const_string!(ToolListChangedNotificationMethod = "notifications/tools/list_changed");
/// Notification sent when the list of available tools changes
pub type ToolListChangedNotification = NotificationNoParam<ToolListChangedNotificationMethod>;

// =============================================================================
// LOGGING
// =============================================================================

/// Logging levels supported by the MCP protocol
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Copy)]
#[serde(rename_all = "lowercase")] //match spec
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum LoggingLevel {
    Debug,
    Info,
    Notice,
    Warning,
    Error,
    Critical,
    Alert,
    Emergency,
}

const_string!(SetLevelRequestMethod = "logging/setLevel");
/// Parameters for setting the logging level
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct SetLevelRequestParam {
    /// The desired logging level
    pub level: LoggingLevel,
}
/// Request to set the logging level
pub type SetLevelRequest = Request<SetLevelRequestMethod, SetLevelRequestParam>;

const_string!(LoggingMessageNotificationMethod = "notifications/message");
/// Parameters for a logging message notification
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct LoggingMessageNotificationParam {
    /// The severity level of this log message
    pub level: LoggingLevel,
    /// Optional logger name that generated this message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logger: Option<String>,
    /// The actual log data
    pub data: Value,
}
/// Notification containing a log message
pub type LoggingMessageNotification =
    Notification<LoggingMessageNotificationMethod, LoggingMessageNotificationParam>;

// =============================================================================
// SAMPLING (LLM INTERACTION)
// =============================================================================

const_string!(CreateMessageRequestMethod = "sampling/createMessage");
pub type CreateMessageRequest = Request<CreateMessageRequestMethod, CreateMessageRequestParam>;

/// Represents the role of a participant in a conversation or message exchange.
///
/// Used in sampling and chat contexts to distinguish between different
/// types of message senders in the conversation flow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum Role {
    /// A human user or client making a request
    User,
    /// An AI assistant or server providing a response
    Assistant,
}

/// A message in a sampling conversation, containing a role and content.
///
/// This represents a single message in a conversation flow, used primarily
/// in LLM sampling requests where the conversation history is important
/// for generating appropriate responses.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct SamplingMessage {
    /// The role of the message sender (User or Assistant)
    pub role: Role,
    /// The actual content of the message (text, image, etc.)
    pub content: Content,
}

/// Specifies how much context should be included in sampling requests.
///
/// This allows clients to control what additional context information
/// should be provided to the LLM when processing sampling requests.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum ContextInclusion {
    /// Include context from all connected MCP servers
    #[serde(rename = "allServers")]
    AllServers,
    /// Include no additional context
    #[serde(rename = "none")]
    None,
    /// Include context only from the requesting server
    #[serde(rename = "thisServer")]
    ThisServer,
}

/// Parameters for creating a message through LLM sampling.
///
/// This structure contains all the necessary information for a client to
/// generate an LLM response, including conversation history, model preferences,
/// and generation parameters.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct CreateMessageRequestParam {
    /// The conversation history and current messages
    pub messages: Vec<SamplingMessage>,
    /// Preferences for model selection and behavior
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_preferences: Option<ModelPreferences>,
    /// System prompt to guide the model's behavior
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// How much context to include from MCP servers
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_context: Option<ContextInclusion>,
    /// Temperature for controlling randomness (0.0 to 1.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Maximum number of tokens to generate
    pub max_tokens: u32,
    /// Sequences that should stop generation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    /// Additional metadata for the request
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

/// Preferences for model selection and behavior in sampling requests.
///
/// This allows servers to express their preferences for which model to use
/// and how to balance different priorities when the client has multiple
/// model options available.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ModelPreferences {
    /// Specific model names or families to prefer (e.g., "claude", "gpt")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hints: Option<Vec<ModelHint>>,
    /// Priority for cost optimization (0.0 to 1.0, higher = prefer cheaper models)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_priority: Option<f32>,
    /// Priority for speed/latency (0.0 to 1.0, higher = prefer faster models)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_priority: Option<f32>,
    /// Priority for intelligence/capability (0.0 to 1.0, higher = prefer more capable models)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intelligence_priority: Option<f32>,
}

/// A hint suggesting a preferred model name or family.
///
/// Model hints are advisory suggestions that help clients choose appropriate
/// models. They can be specific model names or general families like "claude" or "gpt".
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ModelHint {
    /// The suggested model name or family identifier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

// =============================================================================
// COMPLETION AND AUTOCOMPLETE
// =============================================================================

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct CompleteRequestParam {
    pub r#ref: Reference,
    pub argument: ArgumentInfo,
}

pub type CompleteRequest = Request<CompleteRequestMethod, CompleteRequestParam>;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct CompletionInfo {
    pub values: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_more: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct CompleteResult {
    pub completion: CompletionInfo,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "type")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum Reference {
    #[serde(rename = "ref/resource")]
    Resource(ResourceReference),
    #[serde(rename = "ref/prompt")]
    Prompt(PromptReference),
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ResourceReference {
    pub uri: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct PromptReference {
    pub name: String,
}

const_string!(CompleteRequestMethod = "completion/complete");
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ArgumentInfo {
    pub name: String,
    pub value: String,
}

// =============================================================================
// ROOTS AND WORKSPACE MANAGEMENT
// =============================================================================

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct Root {
    pub uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

const_string!(ListRootsRequestMethod = "roots/list");
pub type ListRootsRequest = RequestNoParam<ListRootsRequestMethod>;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ListRootsResult {
    pub roots: Vec<Root>,
}

const_string!(RootsListChangedNotificationMethod = "notifications/roots/list_changed");
pub type RootsListChangedNotification = NotificationNoParam<RootsListChangedNotificationMethod>;

// =============================================================================
// TOOL EXECUTION RESULTS
// =============================================================================

/// The result of a tool call operation.
///
/// Contains the content returned by the tool execution and an optional
/// flag indicating whether the operation resulted in an error.
#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct CallToolResult {
    /// The content returned by the tool (text, images, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<Content>>,
    /// An optional JSON object that represents the structured result of the tool call
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Value>,
    /// Whether this result represents an error condition
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

impl CallToolResult {
    /// Create a successful tool result with unstructured content
    pub fn success(content: Vec<Content>) -> Self {
        CallToolResult {
            content: Some(content),
            structured_content: None,
            is_error: Some(false),
        }
    }
    /// Create an error tool result with unstructured content
    pub fn error(content: Vec<Content>) -> Self {
        CallToolResult {
            content: Some(content),
            structured_content: None,
            is_error: Some(true),
        }
    }
    /// Create a successful tool result with structured content
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use rmcp::model::CallToolResult;
    /// use serde_json::json;
    ///
    /// let result = CallToolResult::structured(json!({
    ///     "temperature": 22.5,
    ///     "humidity": 65,
    ///     "description": "Partly cloudy"
    /// }));
    /// ```
    pub fn structured(value: Value) -> Self {
        CallToolResult {
            content: None,
            structured_content: Some(value),
            is_error: Some(false),
        }
    }
    /// Create an error tool result with structured content
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use rmcp::model::CallToolResult;
    /// use serde_json::json;
    ///
    /// let result = CallToolResult::structured_error(json!({
    ///     "error_code": "INVALID_INPUT",
    ///     "message": "Temperature value out of range",
    ///     "details": {
    ///         "min": -50,
    ///         "max": 50,
    ///         "provided": 100
    ///     }
    /// }));
    /// ```
    pub fn structured_error(value: Value) -> Self {
        CallToolResult {
            content: None,
            structured_content: Some(value),
            is_error: Some(true),
        }
    }

    /// Validate that content or structured content is provided
    pub fn validate(&self) -> Result<(), &'static str> {
        match (&self.content, &self.structured_content) {
            (None, None) => Err("either content or structured_content must be provided"),
            _ => Ok(()),
        }
    }
}

// Custom deserialize implementation to validate mutual exclusivity
impl<'de> Deserialize<'de> for CallToolResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct CallToolResultHelper {
            #[serde(skip_serializing_if = "Option::is_none")]
            content: Option<Vec<Content>>,
            #[serde(skip_serializing_if = "Option::is_none")]
            structured_content: Option<Value>,
            #[serde(skip_serializing_if = "Option::is_none")]
            is_error: Option<bool>,
        }

        let helper = CallToolResultHelper::deserialize(deserializer)?;
        let result = CallToolResult {
            content: helper.content,
            structured_content: helper.structured_content,
            is_error: helper.is_error,
        };

        // Validate mutual exclusivity
        result.validate().map_err(serde::de::Error::custom)?;

        Ok(result)
    }
}

const_string!(ListToolsRequestMethod = "tools/list");
/// Request to list all available tools from a server
pub type ListToolsRequest = RequestOptionalParam<ListToolsRequestMethod, PaginatedRequestParam>;

paginated_result!(
    ListToolsResult {
        tools: Vec<Tool>
    }
);

const_string!(CallToolRequestMethod = "tools/call");
/// Parameters for calling a tool provided by an MCP server.
///
/// Contains the tool name and optional arguments needed to execute
/// the tool operation.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct CallToolRequestParam {
    /// The name of the tool to call
    pub name: Cow<'static, str>,
    /// Arguments to pass to the tool (must match the tool's input schema)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<JsonObject>,
}

/// Request to call a specific tool
pub type CallToolRequest = Request<CallToolRequestMethod, CallToolRequestParam>;

/// The result of a sampling/createMessage request containing the generated response.
///
/// This structure contains the generated message along with metadata about
/// how the generation was performed and why it stopped.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct CreateMessageResult {
    /// The identifier of the model that generated the response
    pub model: String,
    /// The reason why generation stopped (e.g., "endTurn", "maxTokens")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    /// The generated message with role and content
    #[serde(flatten)]
    pub message: SamplingMessage,
}

impl CreateMessageResult {
    pub const STOP_REASON_END_TURN: &str = "endTurn";
    pub const STOP_REASON_END_SEQUENCE: &str = "stopSequence";
    pub const STOP_REASON_END_MAX_TOKEN: &str = "maxTokens";
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct GetPromptResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub messages: Vec<PromptMessage>,
}

// =============================================================================
// MESSAGE TYPE UNIONS
// =============================================================================

macro_rules! ts_union {
    (
        export type $U: ident =
            $(|)?$($V: ident)|*;
    ) => {
        #[derive(Debug, Serialize, Deserialize, Clone)]
        #[serde(untagged)]
        #[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
        pub enum $U {
            $($V($V),)*
        }
    };
}

ts_union!(
    export type ClientRequest =
    | PingRequest
    | InitializeRequest
    | CompleteRequest
    | SetLevelRequest
    | GetPromptRequest
    | ListPromptsRequest
    | ListResourcesRequest
    | ListResourceTemplatesRequest
    | ReadResourceRequest
    | SubscribeRequest
    | UnsubscribeRequest
    | CallToolRequest
    | ListToolsRequest;
);

ts_union!(
    export type ClientNotification =
    | CancelledNotification
    | ProgressNotification
    | InitializedNotification
    | RootsListChangedNotification;
);

ts_union!(
    export type ClientResult = CreateMessageResult | ListRootsResult | EmptyResult;
);

impl ClientResult {
    pub fn empty(_: ()) -> ClientResult {
        ClientResult::EmptyResult(EmptyResult {})
    }
}

pub type ClientJsonRpcMessage = JsonRpcMessage<ClientRequest, ClientResult, ClientNotification>;

ts_union!(
    export type ServerRequest =
    | PingRequest
    | CreateMessageRequest
    | ListRootsRequest;
);

ts_union!(
    export type ServerNotification =
    | CancelledNotification
    | ProgressNotification
    | LoggingMessageNotification
    | ResourceUpdatedNotification
    | ResourceListChangedNotification
    | ToolListChangedNotification
    | PromptListChangedNotification;
);

ts_union!(
    export type ServerResult =
    | InitializeResult
    | CompleteResult
    | GetPromptResult
    | ListPromptsResult
    | ListResourcesResult
    | ListResourceTemplatesResult
    | ReadResourceResult
    | CallToolResult
    | ListToolsResult
    | EmptyResult
    ;
);

impl ServerResult {
    pub fn empty(_: ()) -> ServerResult {
        ServerResult::EmptyResult(EmptyResult {})
    }
}

pub type ServerJsonRpcMessage = JsonRpcMessage<ServerRequest, ServerResult, ServerNotification>;

impl TryInto<CancelledNotification> for ServerNotification {
    type Error = ServerNotification;
    fn try_into(self) -> Result<CancelledNotification, Self::Error> {
        if let ServerNotification::CancelledNotification(t) = self {
            Ok(t)
        } else {
            Err(self)
        }
    }
}

impl TryInto<CancelledNotification> for ClientNotification {
    type Error = ClientNotification;
    fn try_into(self) -> Result<CancelledNotification, Self::Error> {
        if let ClientNotification::CancelledNotification(t) = self {
            Ok(t)
        } else {
            Err(self)
        }
    }
}
impl From<CancelledNotification> for ServerNotification {
    fn from(value: CancelledNotification) -> Self {
        ServerNotification::CancelledNotification(value)
    }
}

impl From<CancelledNotification> for ClientNotification {
    fn from(value: CancelledNotification) -> Self {
        ClientNotification::CancelledNotification(value)
    }
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn test_notification_serde() {
        let raw = json!( {
            "jsonrpc": JsonRpcVersion2_0,
            "method": InitializedNotificationMethod,
        });
        let message: ClientJsonRpcMessage =
            serde_json::from_value(raw.clone()).expect("invalid notification");
        match &message {
            ClientJsonRpcMessage::Notification(JsonRpcNotification {
                notification: ClientNotification::InitializedNotification(_n),
                ..
            }) => {}
            _ => panic!("Expected Notification"),
        }
        let json = serde_json::to_value(message).expect("valid json");
        assert_eq!(json, raw);
    }

    #[test]
    fn test_request_conversion() {
        let raw = json!( {
            "jsonrpc": JsonRpcVersion2_0,
            "id": 1,
            "method": "request",
            "params": {"key": "value"},
        });
        let message: JsonRpcMessage = serde_json::from_value(raw.clone()).expect("invalid request");

        match &message {
            JsonRpcMessage::Request(r) => {
                assert_eq!(r.id, RequestId::Number(1));
                assert_eq!(r.request.method, "request");
                assert_eq!(
                    &r.request.params,
                    json!({"key": "value"})
                        .as_object()
                        .expect("should be an object")
                );
            }
            _ => panic!("Expected Request"),
        }
        let json = serde_json::to_value(&message).expect("valid json");
        assert_eq!(json, raw);
    }

    #[test]
    fn test_initial_request_response_serde() {
        let request = json!({
          "jsonrpc": "2.0",
          "id": 1,
          "method": "initialize",
          "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {
              "roots": {
                "listChanged": true
              },
              "sampling": {}
            },
            "clientInfo": {
              "name": "ExampleClient",
              "version": "1.0.0"
            }
          }
        });
        let raw_response_json = json!({
          "jsonrpc": "2.0",
          "id": 1,
          "result": {
            "protocolVersion": "2024-11-05",
            "capabilities": {
              "logging": {},
              "prompts": {
                "listChanged": true
              },
              "resources": {
                "subscribe": true,
                "listChanged": true
              },
              "tools": {
                "listChanged": true
              }
            },
            "serverInfo": {
              "name": "ExampleServer",
              "version": "1.0.0"
            }
          }
        });
        let request: ClientJsonRpcMessage =
            serde_json::from_value(request.clone()).expect("invalid request");
        let (request, id) = request.into_request().expect("should be a request");
        assert_eq!(id, RequestId::Number(1));
        match request {
            ClientRequest::InitializeRequest(Request {
                method: _,
                params:
                    InitializeRequestParam {
                        protocol_version: _,
                        capabilities,
                        client_info,
                    },
                ..
            }) => {
                assert_eq!(capabilities.roots.unwrap().list_changed, Some(true));
                assert_eq!(capabilities.sampling.unwrap().len(), 0);
                assert_eq!(client_info.name, "ExampleClient");
                assert_eq!(client_info.version, "1.0.0");
            }
            _ => panic!("Expected InitializeRequest"),
        }
        let server_response: ServerJsonRpcMessage =
            serde_json::from_value(raw_response_json.clone()).expect("invalid response");
        let (response, id) = server_response
            .clone()
            .into_response()
            .expect("expect response");
        assert_eq!(id, RequestId::Number(1));
        match response {
            ServerResult::InitializeResult(InitializeResult {
                protocol_version: _,
                capabilities,
                server_info,
                instructions,
            }) => {
                assert_eq!(capabilities.logging.unwrap().len(), 0);
                assert_eq!(capabilities.prompts.unwrap().list_changed, Some(true));
                assert_eq!(
                    capabilities.resources.as_ref().unwrap().subscribe,
                    Some(true)
                );
                assert_eq!(capabilities.resources.unwrap().list_changed, Some(true));
                assert_eq!(capabilities.tools.unwrap().list_changed, Some(true));
                assert_eq!(server_info.name, "ExampleServer");
                assert_eq!(server_info.version, "1.0.0");
                assert_eq!(instructions, None);
            }
            other => panic!("Expected InitializeResult, got {other:?}"),
        }

        let server_response_json: Value = serde_json::to_value(&server_response).expect("msg");

        assert_eq!(server_response_json, raw_response_json);
    }

    #[test]
    fn test_protocol_version_order() {
        let v1 = ProtocolVersion::V_2024_11_05;
        let v2 = ProtocolVersion::V_2025_03_26;
        assert!(v1 < v2);
    }
}
