//! Python bindings for transport types.
//!
//! This module exposes Rust transport types (TCP, stdio, SSE, etc.) to Python via pyo3 bindings.
//!
//! # Examples
//!
//! ```python
//! from rmcp_python import PyTransport
//! transport = PyTransport.from_tcp('127.0.0.1:1234')
//! ```


// Python bindings transport module: re-exports transport types for Python users
pub mod sse;
pub mod child_process;
pub mod io;
pub mod ws;

use pyo3::prelude::*;
use pyo3::exceptions::PyRuntimeError;
use pyo3_asyncio::tokio::future_into_py;
use tokio::net::TcpStream;
use std::pin::Pin;

pub enum PyTransportEnum {
    Tcp(Pin<Box<TcpStream>>),
    Stdio((tokio::io::Stdin, tokio::io::Stdout)),
    Sse(rmcp::transport::SseTransport<rmcp::transport::sse::ReqwestSseClient, reqwest::Error>),
}

/// Python-exposed transport handle for RMCP communication.
#[pyclass]
pub struct PyTransport {
    /// The underlying transport enum (TCP, stdio, SSE, etc.).
    pub(crate) inner: Option<PyTransportEnum>,
}

#[pymethods]
impl PyTransport {
    /// Create a new TCP transport from the given address.
    ///
    /// # Arguments
    /// * `addr` - Address to connect to.
    ///
    /// # Examples
    /// ```python
    /// transport = PyTransport.from_tcp('127.0.0.1:1234')
    /// ```
    #[staticmethod]
    fn from_tcp(py: Python, addr: String) -> PyResult<&PyAny> {
        future_into_py(py, async move {
            match TcpStream::connect(addr).await {
                Ok(stream) => Ok(PyTransport {
                    inner: Some(PyTransportEnum::Tcp(Box::pin(stream))),
                }),
                Err(e) => Err(PyRuntimeError::new_err(e.to_string())),
            }
        })
    }

    #[staticmethod]
    fn from_stdio(py: Python) -> PyResult<&PyAny> {
        future_into_py(py, async move {
            let stdin = tokio::io::stdin();
            let stdout = tokio::io::stdout();
            Ok(PyTransport {
                inner: Some(PyTransportEnum::Stdio((stdin, stdout))),
            })
        })
    }
    
    #[staticmethod]
    pub fn from_sse(py_sse: &mut PySseTransport) -> Self {
        PyTransport {
            inner: Some(PyTransportEnum::Sse(
                py_sse.inner.take().expect("SSE transport already taken"),
            )),
        }
    }

    pub fn is_tcp(&self) -> bool {
        matches!(self.inner, Some(PyTransportEnum::Tcp(_)))
    }
    pub fn is_stdio(&self) -> bool {
        matches!(self.inner, Some(PyTransportEnum::Stdio(_)))
    }
   
    // Add more utility methods as needed
}

// Re-export for Python users
// you can add #[cfg(feature = "python")] if you want to gate these for Python only
pub use sse::*;
pub use child_process::*;
