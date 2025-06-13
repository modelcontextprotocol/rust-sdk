//! Python bindings for core RMCP types.
//!
//! This module exposes Rust types (requests, notifications, content, roles, etc.) to Python via pyo3 bindings.
//!
//! # Overview
//!
//! The types in this module are designed to bridge the RMCP SDK's Rust data structures with Python, allowing Python users to construct, inspect, and manipulate RMCP protocol messages and objects in a type-safe and ergonomic way. Each struct is annotated for Python interop and provides methods for conversion to and from Python-native types (such as `dict`), as well as convenience constructors and helpers.
//!
//! # Main Types
//!
//! - [`PyRoot`]: Represents a root entity in the RMCP protocol.
//! - [`PyRequest`], [`PyNotification`]: Represent protocol requests and notifications, with JSON/dict conversion helpers.
//! - [`PyCreateMessageParams`]: Parameters for creating a new message.
//! - [`PyRole`]: Represents the role of a participant (e.g. user, assistant).
//! - [`PyTextContent`], [`PyImageContent`], [`PyEmbeddedResourceContent`], [`PyAudioContent`]: Content types for message payloads.
//! - [`PyClientInfo`]: Information about the RMCP client, including protocol version and capabilities.
//!
//! # Example Usage
//!
//! ```python
//! from rmcp_python import PyRoot, PyCreateMessageParams, PyRole, PyTextContent
//!
//! # Create a root entity
//! root = PyRoot(id='root1', name='Root')
//!
//! # Create message parameters
//! params = PyCreateMessageParams(content='Hello!', temperature=0.7, max_tokens=128)
//!
//! # Define a role
//! role = PyRole('assistant')
//!
//! # Create text content
//! text = PyTextContent('Hello, world!')
//! ```
#![allow(non_local_definitions)]

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyAny};
use pyo3::exceptions::{PyValueError, PyRuntimeError};
use pyo3::{Python, Py, PyObject};
use pyo3::types::PyType;
use std::borrow::Cow;
use serde::{Deserialize, Serialize};
use serde_json::{Value, Map};
use tokio_util::sync::CancellationToken;
use rmcp::model::{CreateMessageRequestParam, CreateMessageResult, ListRootsResult, Root, 
    Content, SamplingMessage, Role, RawContent, RawTextContent, RawImageContent, RawEmbeddedResource, 
    RawAudioContent, ResourceContents, Request, RequestId, Notification, CallToolRequestParam, 
    CallToolResult, ReadResourceRequestParam, ReadResourceResult, GetPromptRequestParam, 
    GetPromptResult, ClientInfo, Implementation, Meta, Extensions, ProtocolVersion};
use rmcp::service::{Peer, RequestContext, RoleClient};
use rmcp::ServiceExt;
use crate::model::prompt::PyPromptMessage;
use crate::model::resource::{PyTextResourceContents, PyBlobResourceContents, PyResourceContents};
use crate::model::capabilities::PyClientCapabilities;

/// Macro to define Python classes for RMCP protocol types with JSON/dict conversion.
///
/// This macro generates a Python class wrapper for the given Rust type, with methods to construct from a Python dict
/// and to convert back to a dict. Used for types like `Request` and `Notification`.
///
/// # Example
/// ```python
/// req = PyRequest({...})
/// as_dict = req.to_dict()
/// ```
// Macro for PyRequest and PyNotification only, using Python json for PyDict conversion
macro_rules! def_pyclass {
    ($pyname:ident, $rusttype:ty) => {
        #[pyclass]
        #[derive(Clone)]
        pub struct $pyname {
            pub inner: $rusttype,
        }
        #[pymethods]
        impl $pyname {
            #[new]
            pub fn new(py: Python, dict: &PyDict) -> PyResult<Self> {
                let json_mod = py.import("json")?;
                let json_str: String = json_mod.call_method1("dumps", (dict,))?.extract()?;
                let inner: $rusttype = serde_json::from_str(&json_str)
                    .map_err(|e| PyValueError::new_err(format!("Invalid {}: {}", stringify!($pyname), e)))?;
                Ok($pyname { inner })
            }
            pub fn to_dict(&self, py: Python) -> PyResult<PyObject> {
                let json_mod = py.import("json")?;
                let json_str = serde_json::to_string(&self.inner)
                    .map_err(|e| PyValueError::new_err(e.to_string()))?;
                let dict = json_mod.call_method1("loads", (json_str,))?;
                Ok(dict.into())
            }
        }
    };
}

def_pyclass!(PyRequest, Request);
def_pyclass!(PyNotification, Notification);

/// Represents a root entity in the RMCP protocol.
///
/// A root is a named resource or workspace that messages and operations are scoped to.
///
/// # Fields
/// - `id`: Unique identifier for the root (e.g., a URI).
/// - `name`: Human-readable name for the root.
///
/// # Example
/// ```python
/// root = PyRoot(id='workspace-123', name='My Workspace')
/// print(root.id, root.name)
/// ```
#[pyclass]
#[derive(Clone)]
pub struct PyRoot {
    /// The unique identifier of the root (e.g., a URI or UUID).
    #[pyo3(get, set)]
    pub id: String,
    /// The human-readable display name of the root.
    #[pyo3(get, set)]
    pub name: String,
}

impl From<Root> for PyRoot {
    fn from(root: Root) -> Self {
        Self {
            id: root.uri,
            name: root.name.unwrap_or_default(),
        }
    }
}

#[pymethods]
impl PyRoot {
    /// Creates a new `PyRoot`.
    ///
    /// # Arguments
    /// * `id` - The unique identifier for the root.
    /// * `name` - The human-readable name.
    ///
    /// # Example
    /// ```python
    /// root = PyRoot(id='workspace-123', name='My Workspace')
    /// ```
    #[new]
    fn new(id: String, name: String) -> Self {
        PyRoot { id, name }
    }
}

/// Parameters for creating a new message in the RMCP protocol.
///
/// Used to configure the content and sampling parameters for message generation.
///
/// # Fields
/// - `content`: The textual content of the message.
/// - `temperature`: (Optional) Sampling temperature for generation. Higher values = more random.
/// - `max_tokens`: (Optional) Maximum number of tokens for the generated message.
///
/// # Example
/// ```python
/// params = PyCreateMessageParams(content='Hello!', temperature=0.8, max_tokens=128)
/// ```
#[pyclass]
#[derive(Clone)]
pub struct PyCreateMessageParams {
    /// The message content to generate.
    #[pyo3(get, set)]
    pub content: String,
    /// Sampling temperature (higher = more random, lower = more deterministic).
    #[pyo3(get, set)]
    pub temperature: Option<f32>,
    /// Maximum number of tokens to generate.
    #[pyo3(get, set)]
    pub max_tokens: Option<u32>,
}

impl From<PyCreateMessageParams> for CreateMessageRequestParam {
    fn from(params: PyCreateMessageParams) -> Self {
        Self {
            messages: vec![],
            model_preferences: None,
            system_prompt: None,
            include_context: None,
            temperature: params.temperature,
            max_tokens: params.max_tokens.unwrap_or(0),
            stop_sequences: None,
            metadata: None,
        }
    }
}

#[pymethods]
impl PyCreateMessageParams {
    #[new]
    fn new(content: String, temperature: Option<f32>, max_tokens: Option<u32>) -> Self {
        PyCreateMessageParams { content, temperature, max_tokens }
    }
}

/// Represents the role of a participant in the conversation.
///
/// Typical roles include "user", "assistant", or custom roles.
///
/// # Example
/// ```python
/// user_role = PyRole('user')
/// assistant_role = PyRole('assistant')
/// ```
#[pyclass]
#[derive(Clone)]
pub struct PyRole {
    #[pyo3(get, set)]
    pub value: String,
}

impl From<Role> for PyRole {
    fn from(role: Role) -> Self {
        Self { value: format!("{:?}", role) }
    }
}

#[pymethods]
impl PyRole {
    #[new]
    fn new(value: String) -> Self {
        PyRole { value }
    }
}

/// Represents text content in a message.
///
/// Used for plain textual messages in the RMCP protocol.
///
/// # Example
/// ```python
/// text = PyTextContent('Hello, world!')
/// print(str(text))
/// ```
#[pyclass]
#[derive(Clone, Debug)]
pub struct PyTextContent {
    #[pyo3(get, set)]
    pub text: String,
}

impl From<RawTextContent> for PyTextContent {
    fn from(raw: RawTextContent) -> Self {
        Self { text: raw.text }
    }
}

#[pymethods]
impl PyTextContent {
    #[new]
    fn new(text: String) -> Self {
        PyTextContent { text }
    }

    fn __str__(&self) -> PyResult<String> {
        Ok(format!("PyTextContent(text=\"{}\")", self.text))
    }
    fn __repr__(&self) -> PyResult<String> {
        Ok(format!("<PyTextContent text=\"{}\">", self.text))
    }
}

/// Represents image content in a message.
///
/// Used for sending image data as part of a message.
///
/// # Fields
/// - `data`: The image data, typically as a base64-encoded string.
/// - `mime_type`: The MIME type of the image (e.g., "image/png").
///
/// # Example
/// ```python
/// img = PyImageContent(data='<base64string>', mime_type='image/png')
/// ```
#[pyclass]
#[derive(Clone)]
pub struct PyImageContent {
    #[pyo3(get, set)]
    pub data: String,
    #[pyo3(get, set)]
    pub mime_type: String,
}

impl From<RawImageContent> for PyImageContent {
    fn from(raw: RawImageContent) -> Self {
        Self { data: raw.data, mime_type: raw.mime_type }
    }
}

#[pymethods]
impl PyImageContent {
    #[new]
    fn new(data: String, mime_type: String) -> Self {
        PyImageContent { data, mime_type }
    }
}

/// Represents an embedded resource (text or binary) in a message.
///
/// Used for including additional resources (such as files) in messages.
///
/// # Fields
/// - `uri`: The resource URI or identifier.
/// - `mime_type`: (Optional) The MIME type of the resource.
/// - `text`: (Optional) Text content, if the resource is textual.
/// - `blob`: (Optional) Binary content, as a base64-encoded string.
///
/// # Example
/// ```python
/// resource = PyEmbeddedResourceContent(uri='file://foo.txt', mime_type='text/plain', text='Hello!')
/// ```
#[pyclass]
#[derive(Clone)]
pub struct PyEmbeddedResourceContent {
    #[pyo3(get, set)]
    pub uri: String,
    #[pyo3(get, set)]
    pub mime_type: Option<String>,
    #[pyo3(get, set)]
    pub text: Option<String>,
    #[pyo3(get, set)]
    pub blob: Option<String>,
}

impl From<RawEmbeddedResource> for PyEmbeddedResourceContent {
    fn from(raw: RawEmbeddedResource) -> Self {
        match &raw.resource {
            ResourceContents::TextResourceContents { uri, mime_type, text } => Self {
                uri: uri.clone(),
                mime_type: mime_type.clone(),
                text: Some(text.clone()),
                blob: None,
            },
            ResourceContents::BlobResourceContents { uri, mime_type, blob } => Self {
                uri: uri.clone(),
                mime_type: mime_type.clone(),
                text: None,
                blob: Some(blob.clone()),
            },
        }
    }
}

#[pymethods]
impl PyEmbeddedResourceContent {
    #[new]
    fn new(uri: String, mime_type: Option<String>, text: Option<String>, blob: Option<String>) -> Self {
        PyEmbeddedResourceContent { uri, mime_type, text, blob }
    }
}

/// Represents audio content in a message.
///
/// Used for sending audio data as part of a message.
///
/// # Fields
/// - `data`: The audio data, typically as a base64-encoded string.
/// - `mime_type`: The MIME type of the audio (e.g., "audio/wav").
///
/// # Example
/// ```python
/// audio = PyAudioContent(data='<base64string>', mime_type='audio/wav')
/// ```
#[pyclass]
#[derive(Clone)]
pub struct PyAudioContent {
    #[pyo3(get, set)]
    pub data: String,
    #[pyo3(get, set)]
    pub mime_type: String,
}

impl From<RawAudioContent> for PyAudioContent {
    fn from(raw: RawAudioContent) -> Self {
        Self { data: raw.data, mime_type: raw.mime_type }
    }
}

#[pymethods]
impl PyAudioContent {
    #[new]
    fn new(data: String, mime_type: String) -> Self {
        PyAudioContent { data, mime_type }
    }
}

#[pyclass]
#[derive(Clone, Debug)]
pub struct PyContent {
    #[pyo3(get, set)]
    pub kind: String,
    #[pyo3(get, set)]
    pub value: PyObject, // PyTextContent, PyImageContent, etc.
}

#[pymethods]
impl PyContent {
    #[new]
    fn new(kind: String, value: pyo3::PyObject) -> Self {
        PyContent { kind, value }
    }

    fn __str__(&self, py: pyo3::Python) -> PyResult<String> {
        let value_str = match self.value.as_ref(py).str() {
            Ok(pystr) => pystr.to_string_lossy().into_owned(),
            Err(_) => "<unprintable value>".to_string(),
        };
        Ok(format!("PyContent(kind={}, value={})", self.kind, value_str))
    }
    fn __repr__(&self, py: pyo3::Python) -> PyResult<String> {
        let value_repr = match self.value.as_ref(py).repr() {
            Ok(pyrepr) => pyrepr.to_string_lossy().into_owned(),
            Err(_) => "<unprintable value>".to_string(),
        };
        Ok(format!("<PyContent kind='{}' value={}>", self.kind, value_repr))
    }
}

impl PyContent {
    pub fn from(py: Python, content: Content) -> Self {
        match content.raw {
            RawContent::Text(raw) => PyContent {
                kind: "text".to_string(),
                value: Py::new(py, PyTextContent::from(raw)).unwrap().into_py(py),
            },
            RawContent::Image(raw) => PyContent {
                kind: "image".to_string(),
                value: Py::new(py, PyImageContent::from(raw)).unwrap().into_py(py),
            },
            RawContent::Resource(raw) => PyContent {
                kind: "resource".to_string(),
                value: Py::new(py, PyEmbeddedResourceContent::from(raw)).unwrap().into_py(py),
            },
            RawContent::Audio(raw) => PyContent {
                kind: "audio".to_string(),
                value: Py::new(py, PyAudioContent::from(raw.raw)).unwrap().into_py(py),
            },
        }
    }
}

#[pyclass]
#[derive(Clone)]
pub struct PySamplingMessage {
    #[pyo3(get, set)]
    pub role: PyRole,
    #[pyo3(get, set)]
    pub content: PyContent,
}

impl PySamplingMessage {
    pub fn from(py: Python, msg: SamplingMessage) -> Self {
        Self {
            role: PyRole::from(msg.role),
            content: PyContent::from(py, msg.content),
        }
    }
}

#[pyclass]
#[derive(Clone)]
pub struct PyCreateMessageResult {
    #[pyo3(get, set)]
    pub model: String,
    #[pyo3(get, set)]
    pub stop_reason: Option<String>,
    #[pyo3(get, set)]
    pub message: PySamplingMessage,
}

impl PyCreateMessageResult {
    pub fn from(py: Python, result: CreateMessageResult) -> Self {
        Self {
            model: result.model,
            stop_reason: result.stop_reason,
            message: PySamplingMessage::from(py, result.message),
        }
    }
}

#[pymethods]
impl PyCreateMessageResult {
}

#[pyclass]
#[derive(Clone)]
pub struct PyListRootsResult {
    #[pyo3(get, set)]
    pub roots: Vec<PyRoot>,
}

impl PyListRootsResult {
    pub fn from(_py: Python, result: ListRootsResult) -> PyListRootsResult {
        PyListRootsResult {
            roots: result.roots.into_iter().map(PyRoot::from).collect(),
        }
    }
}

#[pymethods]
impl PyListRootsResult {
}

// Custom wrapper for RequestContext<RoleClient>
#[pyclass]
#[derive(Clone)]
pub struct PyRequestContext {
    #[pyo3(get, set)]
    pub id: String,
    #[pyo3(get, set)]
    pub meta: PyObject,
    #[pyo3(get, set)]
    pub extensions: PyObject,
    #[pyo3(get, set)]
    pub peer: PyObject,
}

#[pymethods]
impl PyRequestContext {
    #[new]
    pub fn new(id: String, meta: PyObject, extensions: PyObject, peer: PyObject) -> Self {
        PyRequestContext { id, meta, extensions, peer }
    }
    pub fn to_dict(&self, py: Python) -> PyResult<PyObject> {
        let dict = PyDict::new(py);
        dict.set_item("id", &self.id)?;
        dict.set_item("meta", &self.meta)?;
        dict.set_item("extensions", &self.extensions)?;
        dict.set_item("peer", &self.peer)?;
        Ok(dict.into())
    }
}

impl PyRequestContext {
    pub fn to_rust(&self, py: Python) -> PyResult<RequestContext<RoleClient>> {
        // Convert id (String) to RequestId (type alias for NumberOrString)
        let id: RequestId = if let Ok(num) = self.id.parse::<u32>() {
            RequestId::Number(num)
        } else {
            RequestId::String(self.id.clone().into())
        };

        // Convert meta (PyObject) to Meta (assume JSON dict for now)
        let meta_val = self.meta.as_ref(py);
        let meta: Meta = if let Ok(dict) = meta_val.downcast::<PyDict>() {
            let json_mod = py.import("json")?;
            let json_str: String = json_mod.call_method1("dumps", (dict,))?.extract()?;
            serde_json::from_str(&json_str)
                .map_err(|e| PyValueError::new_err(format!("Invalid meta: {}", e)))?
        } else {
            Meta::default()
        };

        // Convert extensions (PyObject) to Extensions
        // Option 1: Always use Extensions::default() (recommended if you don't need extensions from Python)
        let extensions = Extensions::default();

        // Option 2: Manually insert specific known extension types from Python dict
       

        // Convert peer (PyObject) to Peer<RoleClient>
        let peer_val = self.peer.as_ref(py);
        let peer: Peer<RoleClient> = if let Ok(py_peer) = peer_val.extract::<crate::service::PyPeer>() {
            py_peer.inner
                .as_ref()
                .map(|arc_peer| arc_peer.as_ref().clone())
                .ok_or_else(|| PyValueError::new_err("PyPeer.inner is None"))?
        } else {
            return Err(PyValueError::new_err("peer must be a valid PyPeer"));
        };

        // Use a default CancellationToken for now
        let ct = CancellationToken::new();

        Ok(RequestContext {
            ct,
            id,
            meta,
            extensions,
            peer,
        })
    }
}

#[pyclass]
#[derive(Clone)]
pub struct PyInfo {
    #[pyo3(get)]
    pub protocol_version: String,
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub version: String,
}

#[pymethods]
impl PyInfo {
    fn __str__(&self) -> PyResult<String> {
        Ok(format!("PyInfo(protocol_version=\"{}\", name=\"{}\", version=\"{}\")", self.protocol_version, self.name, self.version))
    }
    fn __repr__(&self) -> PyResult<String> {
        Ok(format!("<PyInfo protocol_version=\"{}\" name=\"{}\" version=\"{}\">", self.protocol_version, self.name, self.version))
    }
}

impl PyInfo {
    pub fn server(info: rmcp::model::InitializeResult) -> Self {
        Self {
            protocol_version: format!("{:?}", info.protocol_version),
            name: info.server_info.name,
            version: info.server_info.version,
        }
    }

    pub fn client(param: rmcp::model::InitializeRequestParam) -> Self {
        Self {
            protocol_version: format!("{:?}", param.protocol_version),
            name: param.client_info.name,
            version: param.client_info.version,
        }
    }
}

/// Python-facing types for client-service API

#[pyclass]
#[derive(Clone)]
pub struct PyCallToolRequestParam {
    #[pyo3(get, set)]
    pub name: String,
    #[pyo3(get, set)]
    pub arguments: Option<PyObject>,
}

impl PyCallToolRequestParam {
    pub fn to_rust(&self, py: Python) -> PyResult<CallToolRequestParam> {
        let arguments: Option<Map<String, Value>> = match &self.arguments {
            Some(obj) => {
                // Convert Python object to JSON string using Python's json.dumps, then parse with serde_json
                let json_mod = py.import("json")?;
                let json_str: String = json_mod.call_method1("dumps", (obj,))?.extract()?;
                let value: Value = serde_json::from_str(&json_str)
                    .map_err(|e| PyValueError::new_err(format!("Failed to parse arguments as JSON: {}", e)))?;
                match value {
                    Value::Object(map) => Some(map),
                    _ => return Err(PyValueError::new_err("Expected dict for arguments")),
                }
            }
            None => None,
        };
        Ok(CallToolRequestParam {
            name: Cow::Owned(self.name.clone()),
            arguments,
        })
    }
}

#[pyclass]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PyReadResourceRequestParam {
    #[pyo3(get, set)]
    pub uri: String,
}

impl From<PyReadResourceRequestParam> for ReadResourceRequestParam {
    fn from(py: PyReadResourceRequestParam) -> Self {
        ReadResourceRequestParam {
            uri: py.uri,
        }
    }
}

#[pyclass]
#[derive(Clone)]
pub struct PyGetPromptRequestParam {
    #[pyo3(get, set)]
    pub name: String,
    #[pyo3(get, set)]
    pub arguments: Option<PyObject>,
}

impl PyGetPromptRequestParam {
    pub fn to_rust(&self, py: Python) -> PyResult<GetPromptRequestParam> {
        let arguments: Option<Map<String, Value>> = match &self.arguments {
            Some(obj) => {
                let json_mod = py.import("json")?;
                let json_str: String = json_mod.call_method1("dumps", (obj,))?.extract()?;
                let value: Value = serde_json::from_str(&json_str)
                    .map_err(|e| PyValueError::new_err(format!("Failed to parse arguments as JSON: {}", e)))?;
                match value {
                    Value::Object(map) => Some(map),
                    _ => return Err(PyValueError::new_err("Expected dict for arguments")),
                }
            }
            None => None,
        };
        Ok(GetPromptRequestParam {
            name: self.name.clone(),
            arguments,
        })
    }
}

// --- Result Types ---

#[pyclass]
#[derive(Clone)]
pub struct PyCallToolResult {
    #[pyo3(get, set)]
    pub content: Vec<PyContent>,
    #[pyo3(get, set)]
    pub is_error: Option<bool>,
}

#[pymethods]
impl PyCallToolResult {
    fn __str__(&self) -> PyResult<String> {
        // Use Debug formatting for each content item for maximum detail
        let content_strs: Vec<String> = self.content.iter().map(|c| format!("{:?}", c)).collect();
        Ok(format!("PyCallToolResult(content=[{}], is_error={:?})", content_strs.join(", "), self.is_error))
    }
    fn __repr__(&self) -> PyResult<String> {
        let content_strs: Vec<String> = self.content.iter().map(|c| format!("{:?}", c)).collect();
        Ok(format!("<PyCallToolResult content=[{}] is_error={:?}>", content_strs.join(", "), self.is_error))
    }
}

impl PyCallToolResult {
    pub fn from(py: Python, res: CallToolResult) -> Self {
        PyCallToolResult {
            content: res.content.into_iter().map(|c| PyContent::from(py, c)).collect(),
            is_error: res.is_error,
        }
    }
}

#[pyclass]
#[derive(Clone)]
pub struct PyReadResourceResult {
    #[pyo3(get, set)]
    pub contents: Vec<Py<PyAny>>,
}

impl PyReadResourceResult {
    pub fn from(py: Python, res: ReadResourceResult) -> Self {
        PyReadResourceResult {
            contents: res.contents.into_iter().map(|item| {
                match item {
                    ResourceContents::TextResourceContents { uri, mime_type, text } => {
                        Py::new(
                            py,
                            (PyTextResourceContents { uri, text, mime_type }, PyResourceContents)
                        ).unwrap().into_py(py)
                    }
                    ResourceContents::BlobResourceContents { uri, mime_type, blob } => {
                        Py::new(
                            py,
                            (PyBlobResourceContents { uri, blob, mime_type }, PyResourceContents)
                        ).unwrap().into_py(py)
                    }
                }
            }).collect(),
        }
    }
}

#[pyclass]
#[derive(Clone)]
pub struct PyGetPromptResult {
    #[pyo3(get, set)]
    pub description: Option<String>,
    #[pyo3(get, set)]
    pub messages: Vec<Py<PyPromptMessage>>,
}

impl PyGetPromptResult {
    pub fn from(py: Python, res: GetPromptResult) -> Self {
        PyGetPromptResult {
            description: res.description,
            messages: res.messages.into_iter().map(|m| {
                Py::new(py, PyPromptMessage {
                    role: format!("{:?}", m.role),
                    content: format!("{:?}", m.content),
                }).unwrap()
            }).collect(),
        }
    }
}


// --- PyImplementation ---
#[pyclass]
#[derive(Clone, Debug, PartialEq)]
pub struct PyImplementation {
    #[pyo3(get, set)]
    pub name: String,
    #[pyo3(get, set)]
    pub version: String,
}

#[pymethods]
impl PyImplementation {
    #[new]
    pub fn new(name: String, version: String) -> Self {
        PyImplementation { name, version }
    }
    
    #[classmethod]
    pub fn from_build_env(_cls: &PyType) -> Self {
        PyImplementation {
            name: env!("CARGO_CRATE_NAME").to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

impl PyImplementation {
    pub fn from(_py: Python, imp: Implementation) -> Self {
        Self {
            name: imp.name,
            version: imp.version,
        }
    }
}

impl From<PyImplementation> for Implementation {
    fn from(implementation: PyImplementation) -> Self {
        Implementation { name: implementation.name, version: implementation.version }
    }
}

impl Default for PyImplementation {
    fn default() -> Self {
        PyImplementation {
            name: env!("CARGO_CRATE_NAME").to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

/// Information about the RMCP client, protocol version, and capabilities.
///
/// Used to describe the client when establishing a connection or serving requests.
///
/// # Example
/// ```python
/// from rmcp_python import PyClientInfo, PyClientCapabilities, PyImplementation
/// info = PyClientInfo(protocol_version='1.0', capabilities=PyClientCapabilities(), client_info=PyImplementation())
/// ```
#[pyclass]
#[derive(Clone)]
pub struct PyClientInfo {
    pub inner: ClientInfo,
}

#[pymethods]
impl PyClientInfo {
    #[new]
    pub fn new(
        protocol_version: String,
        capabilities: PyClientCapabilities,
        client_info: PyImplementation,
    ) -> Self {
        let inner = ClientInfo {
            protocol_version: ProtocolVersion::from(protocol_version),
            capabilities: capabilities.into(),
            client_info: client_info.into(),
        };
        Self { inner }
    }

    pub fn serve<'py>(&self, py: Python<'py>, transport: &mut crate::transport::PyTransport) -> PyResult<&'py PyAny> {
    println!("[PyClientInfo.serve] called with protocol_version={:?}, client_info={:?}", self.inner.protocol_version, self.inner.client_info);
    println!("[PyClientInfo.serve] Transport type: {:?}", transport.inner.as_ref().map(|t| match t {
        crate::transport::PyTransportEnum::Tcp(_) => "Tcp",
        crate::transport::PyTransportEnum::Stdio(_) => "Stdio",
        crate::transport::PyTransportEnum::Sse(_) => "Sse",
    }));
        let info = self.inner.clone();
        match transport.inner.take() {
            Some(crate::transport::PyTransportEnum::Tcp(stream)) => {
                pyo3_asyncio::tokio::future_into_py(py, async move {
                println!("[PyClientInfo.serve/Tcp] Awaiting info.serve(stream)...");
                let running = info.serve(stream).await.map_err(|e| PyRuntimeError::new_err(format!("TCP Serve IO error: {}", e)))?;
                println!("[PyClientInfo.serve/Tcp] Got running instance, peer={:?}", running.peer());
                let peer = running.peer().clone();
                let peer_arc = std::sync::Arc::new(peer);
                Python::with_gil(|py| {
                    println!("[PyClientInfo.serve/Tcp] Creating PyClient");
                    let py_client = crate::service::PyClient::new(peer_arc);
                    Ok(Py::new(py, py_client)?.to_object(py))
                })
            })    }
            Some(crate::transport::PyTransportEnum::Stdio(stdin_stdout)) => {
                pyo3_asyncio::tokio::future_into_py(py, async move {
                println!("[PyClientInfo.serve/Stdio] Awaiting info.serve(stdin_stdout)...");
                let running = info.serve(stdin_stdout).await.map_err(|e| PyRuntimeError::new_err(format!("STDIO Serve IO error: {}", e)))?;
                println!("[PyClientInfo.serve/Stdio] Got running instance, peer={:?}", running.peer());
                let peer = running.peer().clone();
                let peer_arc = std::sync::Arc::new(peer);
                Python::with_gil(|py| {
                    println!("[PyClientInfo.serve/Stdio] Creating PyClient");
                    let py_client = crate::service::PyClient::new(peer_arc);
                    Ok(Py::new(py, py_client)?.to_object(py))
                })
            })    }
            Some(crate::transport::PyTransportEnum::Sse(sse)) => {
                pyo3_asyncio::tokio::future_into_py(py, async move {
                println!("[PyClientInfo.serve/Sse] Awaiting info.serve(sse)...");
                let running = info.serve(sse).await.map_err(|e| PyRuntimeError::new_err(format!("SSE Serve IO error: {}", e)))?;
                println!("[PyClientInfo.serve/Sse] Got running instance, peer={:?}", running.peer());
                let peer = running.peer().clone();
                let peer_arc = std::sync::Arc::new(peer);
                Python::with_gil(|py| {
                    println!("[PyClientInfo.serve/Sse] Creating PyClient");
                    let py_client = crate::service::PyClient::new(peer_arc);
                    Ok(Py::new(py, py_client)?.to_object(py))
                })
            })    }
            None => Err(PyRuntimeError::new_err("Transport not initialized")),
        }
    }
}
