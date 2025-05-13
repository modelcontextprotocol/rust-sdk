//! Python bindings for tool-related types (currently commented out).
//!
//! This module was intended to define Rust representations of tools and tool annotations for use in Python bindings.
//! The code is currently commented out, possibly pending future implementation or refactor.
//!
//! # Note
//!
//! If you want to enable this module, uncomment the code and ensure all dependencies and types are available.
//!
//! # Examples
//!
//! ```python
//! # from rmcp_python import PyTool, PyToolAnnotations
//! # tool = PyTool(name='example', description='desc', input_schema={}, annotations=None)
//! ```
/*
use pyo3::prelude::*;
use pyo3::types::PyAny;
use pyo3::types::PyDict;
use pyo3::types::IntoPyDict;
use pyo3::PyObject;
use serde_json::Value;
use rmcp::model::tool::{Tool, ToolAnnotations};

/// Annotations for a tool, exposed to Python.
#[pyclass]
#[derive(Clone, Debug)]
pub struct PyToolAnnotations {
    /// Optional title for the tool.
    #[pyo3(get, set)]
    pub title: Option<String>,
    /// Indicates if the tool is read-only.
    #[pyo3(get, set)]
    pub read_only_hint: Option<bool>,
    /// Indicates if the tool is destructive.
    #[pyo3(get, set)]
    pub destructive_hint: Option<bool>,
    /// Indicates if the tool is idempotent.
    #[pyo3(get, set)]
    pub idempotent_hint: Option<bool>,
    /// Indicates if the tool operates in an open world.
    #[pyo3(get, set)]
    pub open_world_hint: Option<bool>,
}

/// Represents a tool for use in Python bindings.
#[pyclass]
#[derive(Clone, Debug)]
pub struct PyTool {
    /// Name of the tool.
    #[pyo3(get, set)]
    pub name: String,
    /// Optional description of the tool.
    #[pyo3(get, set)]
    pub description: Option<String>,
    /// Input schema as a Python object (dict).
    #[pyo3(get, set)]
    pub input_schema: PyObject, // Expose as Python dict/object
    /// Optional annotations for the tool.
    #[pyo3(get, set)]
    pub annotations: Option<PyToolAnnotations>,
}

impl From<ToolAnnotations> for PyToolAnnotations {
    fn from(ann: ToolAnnotations) -> Self {
        PyToolAnnotations {
            title: ann.title,
            read_only_hint: ann.read_only_hint,
            destructive_hint: ann.destructive_hint,
            idempotent_hint: ann.idempotent_hint,
            open_world_hint: ann.open_world_hint,
        }
    }
}

impl From<Tool> for PyTool {
    fn from(tool: Tool) -> Self {
        Python::with_gil(|py| {
            let input_schema: Value = (*tool.input_schema).clone();
            let input_schema_py = serde_json::to_string(&input_schema)
                .ok()
                .and_then(|s| py.eval(&s, None, None).ok())
                .map(|obj| obj.to_object(py))
                .unwrap_or_else(|| py.None());
            PyTool {
                name: tool.name.into_owned(),
                description: tool.description.map(|c| c.into_owned()),
                input_schema: input_schema_py,
                annotations: tool.annotations.map(PyToolAnnotations::from),
            }
        })
    }
}
*/