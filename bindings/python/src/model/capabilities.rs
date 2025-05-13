//! Python bindings for capabilities-related types.
//!
//! This module defines Rust representations of client and roots capabilities for use in Python bindings.
//!
//! # Examples
//!
//! ```python
//! from rmcp_python import PyClientCapabilities
//! caps = PyClientCapabilities()
//! ```
#![allow(non_local_definitions)]

use pyo3::prelude::*;
use pyo3::{Python, PyObject};
use rmcp::model::{ClientCapabilities, RootsCapabilities};

// --- PyExperimentalCapabilities ---
/// Experimental capabilities for the client, exposed to Python.
#[pyclass]
#[derive(Clone, Debug)]
pub struct PyExperimentalCapabilities {
    /// Inner Python object representing experimental capabilities.
    #[pyo3(get, set)]
    pub inner: Option<PyObject>,
}

#[pymethods]
impl PyExperimentalCapabilities {
    /// Creates a new `PyExperimentalCapabilities` from an optional Python object.
    ///
    /// # Arguments
    /// * `inner` - Optional Python object representing experimental capabilities.
    ///
    /// # Examples
    /// ```python
    /// exp_caps = PyExperimentalCapabilities()
    /// ```
    #[new]
    pub fn new(inner: Option<PyObject>) -> Self {
        PyExperimentalCapabilities { inner }
    }
}

// --- PyClientCapabilities ---
/// Client capabilities for the Python bindings.
#[pyclass]
#[derive(Clone, Debug, Default)]
pub struct PyClientCapabilities {
    /// Experimental capabilities.
    #[pyo3(get, set)]
    pub experimental: Option<PyExperimentalCapabilities>,
    /// Roots capabilities.
    #[pyo3(get, set)]
    pub roots: Option<PyRootsCapabilities>,
    /// Sampling capabilities as a Python object.
    #[pyo3(get, set)]
    pub sampling: Option<PyObject>,
}

#[pymethods]
impl PyClientCapabilities {
    /// Creates a new `PyClientCapabilities`.
    ///
    /// # Arguments
    /// * `experimental` - Optional experimental capabilities.
    /// * `roots` - Optional roots capabilities.
    /// * `sampling` - Optional sampling capabilities as a Python object.
    ///
    /// # Examples
    /// ```python
    /// caps = PyClientCapabilities()
    /// ```
    #[new]
    pub fn new(
        experimental: Option<PyExperimentalCapabilities>,
        roots: Option<PyRootsCapabilities>,
        sampling: Option<PyObject>,
    ) -> Self {
        PyClientCapabilities {
            experimental,
            roots,
            sampling,
        }
    }
}

impl PyClientCapabilities {
    /// Constructs a `PyClientCapabilities` from a Rust `ClientCapabilities`.
    ///
    /// # Arguments
    /// * `py` - The Python interpreter token.
    /// * `caps` - The Rust `ClientCapabilities` struct.
    pub fn from(py: Python, caps: ClientCapabilities) -> Self {
        let experimental = caps.experimental.map(|exp| {
            let json_str = serde_json::to_string(&exp).expect("serialize experimental");
            let json_mod = py.import("json").expect("import json");
            let py_obj = json_mod.call_method1("loads", (json_str,)).expect("json.loads").into();
            PyExperimentalCapabilities { inner: Some(py_obj) }
        });
        let roots = caps.roots.map(|roots| PyRootsCapabilities { list_changed: roots.list_changed });
        let sampling = caps.sampling.map(|s| {
            let json_str = serde_json::to_string(&s).expect("serialize sampling");
            let json_mod = py.import("json").expect("import json");
            json_mod.call_method1("loads", (json_str,)).expect("json.loads").into()
        });
        Self {
            experimental,
            roots,
            sampling,
        }
    }
}

/// Converts from `PyClientCapabilities` to Rust `ClientCapabilities`.
impl From<PyClientCapabilities> for ClientCapabilities {
    fn from(py_caps: PyClientCapabilities) -> Self {
        let experimental = py_caps.experimental.and_then(|py_exp| {
            py_exp.inner.and_then(|obj| {
                Python::with_gil(|py| {
                    let json_mod = py.import("json").ok()?;
                    let json_str = json_mod.call_method1("dumps", (obj,)).ok()?.extract::<String>().ok()?;
                    serde_json::from_str(&json_str).ok()
                })
            })
        });

        let roots = py_caps.roots.map(|py_roots| RootsCapabilities {
            list_changed: py_roots.list_changed,
        });

        let sampling = py_caps.sampling.and_then(|obj| {
            Python::with_gil(|py| {
                let json_mod = py.import("json").ok()?;
                let json_str = json_mod.call_method1("dumps", (obj,)).ok()?.extract::<String>().ok()?;
                serde_json::from_str(&json_str).ok()
            })
        });

        ClientCapabilities {
            experimental,
            roots,
            sampling,
            ..Default::default()
        }
    }
}


// --- PyRootsCapabilities ---
/// Roots capabilities for the client, exposed to Python.
#[pyclass]
#[derive(Clone, Debug)]
pub struct PyRootsCapabilities {
    /// Indicates if the list of roots has changed.
    #[pyo3(get, set)]
    pub list_changed: Option<bool>,
}

#[pymethods]
impl PyRootsCapabilities {
    /// Creates a new `PyRootsCapabilities`.
    ///
    /// # Arguments
    /// * `list_changed` - Optional boolean indicating if the roots list has changed.
    ///
    /// # Examples
    /// ```python
    /// roots_caps = PyRootsCapabilities()
    /// ```
    #[new]
    pub fn new(list_changed: Option<bool>) -> Self {
        PyRootsCapabilities { list_changed }
    }
}
