//! Python bindings for the RMCP SDK.
//!
//! This crate exposes Rust types and services to Python using pyo3 and maturin.
//! It provides bindings for RMCP client, service, transport, and model types.
//!
//! # Usage
//!
//! Install with maturin, then use from Python. Here is a full async client example:
//!
//! ```python
//! import asyncio
//! import logging
//! from rmcp_python import PyClientInfo, PyClientCapabilities, PyImplementation, PyTransport, PySseTransport
//!
//! logging.basicConfig(level=logging.INFO)
//!
//! SSE_URL = "http://localhost:8000/sse"
//!
//! async def main():
//!     # Create the SSE transport
//!     transport = await PySseTransport.start(SSE_URL)
//!     # Wrap in PyTransport
//!     transport = PyTransport.from_sse(transport)
//!     # Set up client info
//!     client_info = PyClientInfo(
//!         protocol_version="2025-03-26",
//!         capabilities=PyClientCapabilities(),
//!         client_info=PyImplementation(name="test python sse client", version="0.0.1")
//!     )
//!     # Serve the client
//!     client = await client_info.serve(transport)
//!     # Print server info
//!     server_info = client.peer_info()
//!     logging.info(f"Connected to server: {server_info}")
//!     # List available tools
//!     tools = await client.list_all_tools()
//!     logging.info(f"Available tools: {tools}")
//!     # Call a tool (example)
//!     result = await client.call_tool("increment", {})
//!     logging.info(f"Tool result: {result}")
//!
//! if __name__ == "__main__":
//!     asyncio.run(main())
//! ```

use pyo3::prelude::*;
use crate::types::{PyRoot, PyCreateMessageParams, PyCreateMessageResult, PyListRootsResult, 
    PySamplingMessage, PyRole, PyTextContent, PyImageContent, PyEmbeddedResourceContent, PyAudioContent, 
    PyContent, PyReadResourceResult, PyCallToolResult, PyCallToolRequestParam, PyReadResourceRequestParam, 
    PyGetPromptRequestParam, PyGetPromptResult, PyClientInfo, PyImplementation};
use crate::model::capabilities::{PyClientCapabilities, PyRootsCapabilities, PyExperimentalCapabilities};
use crate::transport::{PySseTransport, PyChildProcessTransport, PyTransport};

pub mod client;
pub mod types;
pub mod transport;
pub mod model;
pub mod service;

/// Custom error type for Python bindings
#[derive(thiserror::Error, Debug)]
pub enum BindingError {
    #[error("JSON conversion error: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("Python error: {0}")]
    PyErr(#[from] PyErr),
    #[error("RMCP error: {0}")]
    RmcpError(String),
    #[error("Runtime error: {0}")]
    RuntimeError(String),
}

impl From<BindingError> for PyErr {
    fn from(err: BindingError) -> PyErr {
        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(err.to_string())
    }
}

/// Python module initialization
#[pymodule]
fn rmcp_python(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_class::<PyRoot>()?;
    m.add_class::<PyCreateMessageParams>()?;
    m.add_class::<PyCreateMessageResult>()?;
    m.add_class::<PyListRootsResult>()?;
    m.add_class::<PySamplingMessage>()?;
    m.add_class::<PyRole>()?;
    m.add_class::<PyTextContent>()?;
    m.add_class::<PyImageContent>()?;
    m.add_class::<PyEmbeddedResourceContent>()?;
    m.add_class::<PyAudioContent>()?;
    m.add_class::<PyContent>()?;
    m.add_class::<PyTransport>()?;
    m.add_class::<PySseTransport>()?;
    m.add_class::<PyChildProcessTransport>()?;
    m.add_class::<PyClientInfo>()?;
    m.add_class::<PyRootsCapabilities>()?;  
    m.add_class::<PyExperimentalCapabilities>()?;
    m.add_class::<PyClientCapabilities>()?;
    m.add_class::<PyImplementation>()?;
    m.add_class::<PyCallToolRequestParam>()?;
    m.add_class::<PyReadResourceRequestParam>()?;
    m.add_class::<PyGetPromptRequestParam>()?;
    m.add_class::<PyCallToolResult>()?;
    m.add_class::<PyReadResourceResult>()?;
    m.add_class::<PyGetPromptResult>()?;
    m.add_class::<crate::service::PyService>()?;
    m.add_class::<crate::service::PyPeer>()?;
    Ok(())
}
