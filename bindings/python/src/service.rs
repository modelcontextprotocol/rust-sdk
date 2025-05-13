//! Python bindings for RMCP service logic.
//!
//! This module provides the main service interface (`PyService`) and client handle (`PyClient`) for Python users,
//! exposing core RMCP service capabilities and peer management via pyo3 bindings.
//!
//! # Examples
//!
//! ```python
//! from rmcp_python import PyService
//! service = PyService()
//! ```
#![allow(non_local_definitions)]

use pyo3::prelude::*;
use pyo3::exceptions::PyRuntimeError;
use pyo3::types::{PyDict, PyList};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
use serde_json::Value;
use serde_json::to_value;
use crate::types::{PyRequest, PyCreateMessageResult, PyListRootsResult, PyInfo, PyRequestContext, PyCreateMessageParams};
use crate::client::PyClientHandler;
use rmcp::service::{RoleClient, DynService};
use rmcp::model::{CreateMessageRequest, CallToolRequestParam, ServerRequest, ClientResult, NumberOrString, RequestNoParam};
use rmcp::service::RequestContext;
use rmcp::Peer;

// Manual PyDict to serde_json::Value conversion utilities
pub fn pydict_to_serde_value(dict: &PyDict) -> Value {
    let mut map = serde_json::Map::new();
    for (k, v) in dict.iter() {
        let key = k.extract::<String>().unwrap_or_else(|_| k.str().unwrap().to_string());
        let value = python_to_serde_value(v);
        map.insert(key, value);
    }
    serde_json::Value::Object(map)
}

pub fn python_to_serde_value(obj: &pyo3::PyAny) -> Value {
    if let Ok(val) = obj.extract::<bool>() {
        serde_json::Value::Bool(val)
    } else if let Ok(val) = obj.extract::<i64>() {
        serde_json::Value::Number(val.into())
    } else if let Ok(val) = obj.extract::<f64>() {
        serde_json::Value::Number(serde_json::Number::from_f64(val).unwrap())
    } else if let Ok(val) = obj.extract::<String>() {
        serde_json::Value::String(val)
    } else if let Ok(dict) = obj.downcast::<PyDict>() {
        pydict_to_serde_value(dict)
    } else if let Ok(list) = obj.downcast::<PyList>() {
        serde_json::Value::Array(list.iter().map(|item| python_to_serde_value(item)).collect())
    } else {
        serde_json::Value::Null
    }
}

// Utility for converting serde_json::Value to Python object (dict/list)
fn serde_value_to_py(py: Python, value: &Value) -> PyObject {
    let json_mod = py.import("json").expect("import json");
    let json_str = serde_json::to_string(value).expect("to_string");
    json_mod.call_method1("loads", (json_str,)).expect("json.loads").to_object(py)
}

/// Main service object for Python bindings.
///
/// Provides access to RMCP client handler and peer management from Python.
#[pyclass]
pub struct PyService {
    /// The underlying client handler, protected by a mutex for thread safety.
    pub inner: Arc<Mutex<PyClientHandler>>,
}

#[pymethods]
impl PyService {
    /// Creates a new `PyService` instance.
    ///
    /// # Examples
    /// ```python
    /// service = PyService()
    /// ```
    #[new]
    pub fn new() -> Self {
        let handler = PyClientHandler::new();
        Self {
            inner: Arc::new(Mutex::new(handler)),
        }
    }

    /// Gets the current peer as a Python object, if set.
    ///
    /// # Returns
    /// An optional Python object representing the peer.
    pub fn get_peer<'py>(&self, py: Python<'py>) -> PyResult<Option<PyObject>> {
        let guard = self.inner.lock().unwrap();
        if let Some(peer) = guard.get_peer() {
            let mut py_peer = PyPeer::new();
            py_peer.set_inner(peer.into());
            Ok(Some(Py::new(py, py_peer)?.to_object(py)))
        } else {
            Ok(None)
        }
    }

    /// Sets the peer from a Python object.
    ///
    /// # Arguments
    /// * `py_peer` - The Python peer object to set.
    pub fn set_peer(&self, py: Python, py_peer: PyObject) -> PyResult<()> {
        let peer = py_peer.extract::<pyo3::PyRef<PyPeer>>(py)?.inner.clone();
        let mut inner = self.inner.lock().unwrap();
        if let Some(peer_arc) = peer {
            inner.set_peer((*(peer_arc)).clone());
        } else {
            return Python::with_gil(|_py| Err(PyRuntimeError::new_err("Peer not initialized")));
        }
        Ok(())
    }

    /// Gets information about the client as a Python object.
    pub fn get_info<'py>(&self, py: Python<'py>) -> PyResult<PyObject> {
        let info = self.inner.lock().unwrap().get_info();
        let py_info = PyInfo::client(info);
        Ok(Py::new(py, py_info)?.to_object(py))
    }

    #[pyo3(name = "handle_request")]
    pub fn handle_request<'py>(&self, py: Python<'py>, request: &PyRequest, context: PyObject) -> PyResult<&'py PyAny> {
        let inner = self.inner.clone();
        let req = request.inner.clone();
        pyo3_asyncio::tokio::future_into_py(py, async move {
            // Convert context PyObject to RequestContext<RoleClient>
            let ctx = Python::with_gil(|py| {
                let py_ctx: PyRequestContext = context.extract(py)?;
                py_ctx.to_rust(py)
            })?;
            // Use correct ServerRequest variant (CreateMessageRequest)
            let req_json = serde_json::to_value(&req)
                .map_err(|e| PyRuntimeError::new_err(format!("Failed to serialize request: {}", e)))?;
            let typed_req: rmcp::model::Request<rmcp::model::CreateMessageRequestMethod, rmcp::model::CreateMessageRequestParam> =
                serde_json::from_value(req_json)
                    .map_err(|e| PyRuntimeError::new_err(format!("Failed to convert to typed request: {}", e)))?;
            let server_request = rmcp::model::ServerRequest::CreateMessageRequest(typed_req);

            // Lock the mutex only when needed, then immediately drop the guard
            let handler = {
                let binding = inner.lock().unwrap();
                binding.clone()
            };
            let future = handler.handle_request(server_request, ctx);
            match future.await {
                Ok(rmcp::model::ClientResult::CreateMessageResult(result)) => Python::with_gil(|py| Ok(crate::types::PyCreateMessageResult::from(py, result).into_py(py))),
                Ok(rmcp::model::ClientResult::ListRootsResult(result)) => Python::with_gil(|py| Ok(crate::types::PyListRootsResult::from(py, result).into_py(py))),
                Ok(other) => Err(PyRuntimeError::new_err(format!("Unexpected result type: {:?}", other))),
                Err(e) => Err(PyRuntimeError::new_err(e.to_string())),
            }
        })
    }

    #[pyo3(name = "handle_notification")]
    pub fn handle_notification<'py>(&self, py: Python<'py>, notification: &PyDict) -> PyResult<&'py PyAny> {
        let inner = self.inner.clone();
        // Convert PyDict to serde_json::Value
        let notif_json = pydict_to_serde_value(notification);
        // Deserialize to ServerNotification
        let server_notification: rmcp::model::ServerNotification = serde_json::from_value(notif_json)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        pyo3_asyncio::tokio::future_into_py(py, async move {
            let handler = {
                let binding = inner.lock().unwrap();
                binding.clone()
            };
            let result = handler.handle_notification(server_notification).await;
            Python::with_gil(|py| match result {
                Ok(_) => Ok(py.None()),
                Err(e) => Err(PyRuntimeError::new_err(e.to_string())),
            })
        })
    }

    fn create_message<'py>(&self, py: Python<'py>, params: PyCreateMessageParams) -> PyResult<&'py PyAny> {
        let inner = self.inner.clone();
        let request = CreateMessageRequest {
            method: Default::default(),
            params: params.into(),
            extensions: Default::default(),
        };
        let server_request = ServerRequest::CreateMessageRequest(request);
        pyo3_asyncio::tokio::future_into_py(py, async move {
            let peer = {
                let guard = inner.lock().unwrap();
                guard.get_peer()
            };
            if peer.is_none() {
                return Err(pyo3::exceptions::PyRuntimeError::new_err("Client not connected to server"));
            }
            let peer = peer.unwrap();
            let context = RequestContext {
                ct: CancellationToken::new(),
                id: NumberOrString::Number(1),
                meta: Default::default(),
                extensions: Default::default(),
                peer,
            };
            let handler = {
                let binding = inner.lock().unwrap();
                binding.clone()
            };
            let future = handler.handle_request(server_request, context);
            match future.await {
                Ok(ClientResult::CreateMessageResult(result)) => Python::with_gil(|py| Ok(PyCreateMessageResult::from(py, result).into_py(py))),
                Ok(_) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>("Unexpected response type")),
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string())),
            }
        })
    }

    fn list_roots<'py>(&self, py: Python<'py>) -> PyResult<&'py PyAny> {
        let inner = self.inner.clone();
        let server_request = ServerRequest::ListRootsRequest(RequestNoParam {
            method: Default::default(),
            extensions: Default::default(),
        });
        pyo3_asyncio::tokio::future_into_py(py, async move {
            let peer = {
                let guard = inner.lock().unwrap();
                guard.get_peer()
            };
            if peer.is_none() {
                return Err(pyo3::exceptions::PyRuntimeError::new_err("Client not connected to server"));
            }
            let peer = peer.unwrap();
            let context = RequestContext {
                ct: CancellationToken::new(),
                id: NumberOrString::Number(1),
                meta: Default::default(),
                extensions: Default::default(),
                peer,
            };
            let handler = {
                let binding = inner.lock().unwrap();
                binding.clone()
            };
            let future = handler.handle_request(server_request, context);
            match future.await {
                Ok(ClientResult::ListRootsResult(result)) => Python::with_gil(|py| Ok(PyListRootsResult::from(py, result).into_py(py))),
                Ok(_) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>("Unexpected response type")),
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string())),
            }
        })
    }
    /// List all tools available from the peer, as an awaitable Python method
    #[pyo3(name = "list_all_tools")]
    pub fn list_all_tools<'py>(&self, py: Python<'py>) -> PyResult<&'py PyAny> {
        let inner = self.inner.clone();
        pyo3_asyncio::tokio::future_into_py(py, async move {
            let peer = {
                let guard = inner.lock().unwrap();
                guard.get_peer()
            };
            let peer = match peer {
                Some(p) => p,
                None => return Python::with_gil(|_py| Err(PyRuntimeError::new_err("Peer not initialized"))),
            };
            let req = rmcp::model::RequestOptionalParam {
                method: rmcp::model::ListToolsRequestMethod,
                params: None,
                extensions: Default::default(),
            };
            let req = rmcp::model::ClientRequest::ListToolsRequest(req);
            let result = peer.send_request(req).await;
            Python::with_gil(|py| match result {
                Ok(rmcp::model::ServerResult::ListToolsResult(resp)) => {
                    let tools_py = resp.tools.into_iter().map(|tool| {
                        let val = to_value(tool).unwrap();
                        serde_value_to_py(py, &val)
                    }).collect::<Vec<_>>();
                    Ok(PyList::new(py, tools_py).to_object(py))
                },
                Ok(other) => Err(PyRuntimeError::new_err(format!("Unexpected ServerResult variant for list_all_tools: {:?}", other))),
                Err(e) => Err(PyRuntimeError::new_err(e.to_string())),
            })
        })
    }

    /// List tools with optional parameters (filtering, etc.)
    pub fn list_tools<'py>(&self, py: Python<'py>, params: PyObject) -> PyResult<&'py PyAny> {
        let inner = self.inner.clone();
        let params = params;
        pyo3_asyncio::tokio::future_into_py(py, async move {
            let peer = {
                let guard = inner.lock().unwrap();
                guard.get_peer()
            };
            let peer = match peer {
                Some(p) => p,
                None => return Python::with_gil(|_py| Err(PyRuntimeError::new_err("Peer not initialized"))),
            };
            let req_params = Python::with_gil(|py| {
                if let Ok(dict) = params.extract::<&pyo3::types::PyDict>(py) {
                    let value = pydict_to_serde_value(dict);
                    serde_json::from_value::<rmcp::model::PaginatedRequestParam>(value)
                        .map_err(|e| PyRuntimeError::new_err(e.to_string()))
                } else {
                    Ok(rmcp::model::PaginatedRequestParam { cursor: None })
                }
            })?;
            let req = rmcp::model::RequestOptionalParam {
                method: rmcp::model::ListToolsRequestMethod,
                params: Some(req_params),
                extensions: Default::default(),
            };
            let req = rmcp::model::ClientRequest::ListToolsRequest(req);
            let result = peer.send_request(req).await;
            Python::with_gil(|py| match result {
                Ok(rmcp::model::ServerResult::ListToolsResult(resp)) => {
                    let tools_py = resp.tools.into_iter().map(|tool| {
                        let val = to_value(tool).unwrap();
                        serde_value_to_py(py, &val)
                    }).collect::<Vec<_>>();
                    Ok(PyList::new(py, tools_py).to_object(py))
                },
                Ok(other) => Err(PyRuntimeError::new_err(format!("Unexpected ServerResult variant for list_tools: {:?}", other))),
                Err(e) => Err(PyRuntimeError::new_err(e.to_string())),
            })
        })
    }

    /// Ergonomic Python method: call_tool(name, arguments=None)
    #[pyo3(name = "call_tool")]
    pub fn call_tool<'py>(
        &self,
        py: Python<'py>,
        name: String,
        arguments: Option<PyObject>,
    ) -> PyResult<&'py PyAny> {
        let rust_args = if let Some(args) = arguments {
            let dict: &pyo3::types::PyDict = args.extract(py)?;
            let value: serde_json::Value = pydict_to_serde_value(dict);
            value.as_object().cloned()
        } else {
            None
        };
        let param = rmcp::model::CallToolRequestParam {
            name: name.into(),
            arguments: rust_args,
        };
        let call_tool_req = rmcp::model::Request {
            method: rmcp::model::CallToolRequestMethod,
            params: param,
            extensions: Default::default(),
        };
        let inner = self.inner.clone();
        pyo3_asyncio::tokio::future_into_py(py, async move {
            let peer = {
                let guard = inner.lock().unwrap();
                guard.get_peer()
            };
            let peer = match peer {
                Some(p) => p,
                None => return Python::with_gil(|_py| Err(PyRuntimeError::new_err("Peer not initialized"))),
            };
            let req = rmcp::model::ClientRequest::CallToolRequest(call_tool_req);
            let result = peer.send_request(req).await;
            Python::with_gil(|py| match result {
                Ok(rmcp::model::ServerResult::CallToolResult(resp)) => {
                    let py_obj = crate::types::PyCallToolResult::from(py, resp).into_py(py);
                    Ok(py_obj)
                },
                Ok(other) => Err(PyRuntimeError::new_err(format!("Unexpected ServerResult variant for call_tool: {:?}", other))),
                Err(e) => Err(PyRuntimeError::new_err(e.to_string())),
            })
        })
    }

}

// --- PyClient: Python-exposed handle to a live client peer ---
#[pyclass]
pub struct PyClient {
    inner: Option<Arc<Peer<RoleClient>>>,
}

impl PyClient {
    pub(crate) fn new(peer: Arc<Peer<RoleClient>>) -> Self {
        Self {
            inner: Some(peer),
        }
    }
}
#[pymethods]
impl PyClient {
    pub fn peer_info<'py>(&self, py: Python<'py>) -> PyResult<PyObject> {
    // No async, so this is fine
    match &self.inner {
        Some(peer) => {
            let info = peer.peer_info().clone();
            Ok(Py::new(py, crate::types::PyInfo::server(info))?.to_object(py))
        }
        None => Err(PyRuntimeError::new_err("Peer not initialized")),
    }
}

pub fn list_all_tools<'py>(&self, py: Python<'py>) -> PyResult<&'py PyAny> {
    let peer = self.inner.clone();
    pyo3_asyncio::tokio::future_into_py(py, async move {
        match &peer {
            Some(peer) => {
                let result = peer.list_all_tools().await;
                Python::with_gil(|py| match result {
                    Ok(tools) => {
                        let tools_py: Vec<_> = tools.into_iter().map(|tool| {
                            let val = to_value(&tool).unwrap();
                            crate::service::serde_value_to_py(py, &val)
                        }).collect();
                        Ok(PyList::new(py, tools_py).to_object(py))
                    }
                    Err(e) => Err(PyRuntimeError::new_err(e.to_string())),
                })
            }
            None => Python::with_gil(|_py| Err(PyRuntimeError::new_err("Peer not initialized"))),
        }
    })
}

pub fn call_tool<'py>(&self, py: Python<'py>, name: String, arguments: Option<PyObject>) -> PyResult<&'py PyAny> {
    let peer = self.inner.clone();
    // Extract arguments to serde_json::Value before async block
    let rust_args = if let Some(args) = arguments {
    Python::with_gil(|py| -> PyResult<serde_json::Value> {
        let dict: &pyo3::types::PyDict = args.extract(py)?;
        Ok(crate::service::pydict_to_serde_value(dict))
    })?
} else {
    serde_json::Value::Null
};
    pyo3_asyncio::tokio::future_into_py(py, async move {
        match &peer {
            Some(peer) => {
                let param = CallToolRequestParam {
    name: name.clone().into(),
    arguments: rust_args.as_object().cloned(),
};
                let result = peer.call_tool(param).await;
             Python::with_gil(|py| match result {
                    Ok(resp) => {
                        let py_obj = crate::types::PyCallToolResult::from(py, resp).into_py(py);
                        Ok(py_obj)
                    }
                    Err(e) => Err(PyRuntimeError::new_err(e.to_string())),
                })
            }
            None => Python::with_gil(|_py| Err(PyRuntimeError::new_err("Peer not initialized"))),
        }
    })
}

}

#[pyclass]
#[derive(Clone)]
pub struct PyPeer {
    #[pyo3(get)]
    #[allow(dead_code)]
    _dummy: Option<bool>, // Python cannot access the inner peer
    #[allow(dead_code)]
    pub(crate) inner: Option<Arc<Peer<RoleClient>>>,
}

impl PyPeer {
    pub fn set_inner(&mut self, inner: Arc<Peer<RoleClient>>) {
        self.inner = Some(inner);
    }
}

#[pymethods]
impl PyPeer {
    #[new]
    pub fn new() -> Self {
        PyPeer { _dummy: None, inner: None }
    }


   /* pub fn send_request<'py>(&self, py: Python<'py>, request: PyObject) -> PyResult<&'py PyAny> {
        let peer = self.inner.clone();
        let req_obj = request;
        pyo3_asyncio::tokio::future_into_py(py, async move {
            Python::with_gil(|py| {
                let req_rust = req_obj.extract::<crate::types::PyRequest>(py)?.inner.clone();
                match &peer {
                    Some(peer_arc) => {
                        match tokio::runtime::Handle::current().block_on(peer_arc.send_request(req_rust)) {
                            Ok(server_result) => Ok(Py::new(py, server_result).unwrap().to_object(py)),
                            Err(e) => Err(PyRuntimeError::new_err(e.to_string())),
                        }
                    }
                    None => Err(PyRuntimeError::new_err("Peer not initialized")),
                }
            })
        })
    }

    pub fn send_notification<'py>(&self, py: Python<'py>, notification: PyObject) -> PyResult<&'py PyAny> {
        let peer = self.inner.clone();
        let notif = notification;
        pyo3_asyncio::tokio::future_into_py(py, async move {
            Python::with_gil(|py| {
                let notif_rust = notif.extract::<crate::types::PyNotification>(py)?.inner.clone();
                use rmcp::model::{ClientNotification, CancelledNotification, ProgressNotification, InitializedNotification, RootsListChangedNotification};
                use serde_json::to_value;
                // Manual conversion based on method field
                let method = notif_rust.method.clone();
                let as_value = to_value(&notif_rust).map_err(|e| pyo3::exceptions::PyTypeError::new_err(e.to_string()))?;
                let client_notif = if method == CancelledNotificationMethod::VALUE {
                    let typed: CancelledNotification = serde_json::from_value(as_value).map_err(|e| pyo3::exceptions::PyTypeError::new_err(e.to_string()))?;
                    ClientNotification::CancelledNotification(typed)
                } else if method == ProgressNotificationMethod::VALUE {
                    let typed: ProgressNotification = serde_json::from_value(as_value).map_err(|e| pyo3::exceptions::PyTypeError::new_err(e.to_string()))?;
                    ClientNotification::ProgressNotification(typed)
                } else if method == InitializedNotificationMethod::VALUE {
                    let typed = InitializedNotification {
                        method: InitializedNotificationMethod,
                        extensions: Extensions::default(),
                    };
                    ClientNotification::InitializedNotification(typed)
                } else if method == RootsListChangedNotificationMethod::VALUE {
                    let typed = RootsListChangedNotification {
                        method: RootsListChangedNotificationMethod,
                        extensions: Extensions::default(),
                    };
                    ClientNotification::RootsListChangedNotification(typed)
                } else {
                    return Err(pyo3::exceptions::PyTypeError::new_err("Invalid notification type for ClientNotification"));
                };
                match &peer {
                    Some(peer_arc) => {
                        match tokio::runtime::Handle::current().block_on(peer_arc.send_notification(client_notif)) {
                            Ok(_) => Ok(py.None()),
                            Err(e) => Err(pyo3::exceptions::PyRuntimeError::new_err(e.to_string())),
                        }
                    }
                    None => Err(pyo3::exceptions::PyRuntimeError::new_err("Peer not initialized")),
                }
            })
        })
    }*/
    pub fn peer_info<'py>(&self, py: Python<'py>) -> PyResult<PyObject> {
        match &self.inner {
            Some(peer_arc) => {
                let info = peer_arc.peer_info().clone();
                // PyInfo::new expects InitializeResult, so pass info directly
                Ok(Py::new(py, PyInfo::server(info))?.to_object(py))
            }
            None => Err(pyo3::exceptions::PyRuntimeError::new_err("Peer not initialized")),
        }
    }
}
