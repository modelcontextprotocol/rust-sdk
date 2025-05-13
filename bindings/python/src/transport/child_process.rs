#![allow(non_local_definitions)]

use pyo3::prelude::*;
use rmcp::transport::TokioChildProcess;
use pyo3::exceptions::PyRuntimeError;

#[pyclass]
pub struct PyChildProcessTransport {
    pub inner: TokioChildProcess,
}

#[pymethods]
impl PyChildProcessTransport {
    #[new]
    fn new(cmd: String, args: Vec<String>) -> PyResult<Self> {
        let mut command = tokio::process::Command::new(cmd);
        command.args(args);
        let transport = TokioChildProcess::new(&mut command)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(Self { inner: transport })
    }
}
