#![allow(non_local_definitions)]

use pyo3::prelude::*;
use rmcp::transport::SseTransport;
use rmcp::transport::sse::ReqwestSseClient;
use pyo3::exceptions::PyRuntimeError;
use reqwest;

#[pyclass]
pub struct PySseTransport {
    pub inner: Option<SseTransport<ReqwestSseClient, reqwest::Error>>,
}

#[pymethods]
impl PySseTransport {
    #[staticmethod]
    #[pyo3(name = "start")]
    pub fn start(py: Python, url: String) -> PyResult<&PyAny> {
        pyo3_asyncio::tokio::future_into_py(py, async move {
            match SseTransport::start(&url).await {
                Ok(transport) => Ok(PySseTransport { inner: Some(transport) }),
                Err(e) => Err(PyRuntimeError::new_err(e.to_string())),
            }
        })
    }
}
