//! Python bindings for resource-related types.
//!
//! This module defines Rust representations of resources for use in Python bindings, including text and blob resource contents.
//!
//! # Examples
//!
//! ```python
//! from rmcp_python import PyTextResourceContents, PyBlobResourceContents
//! text_resource = PyTextResourceContents(uri='file.txt', text='Hello', mime_type='text/plain')
//! ```
#![allow(non_local_definitions)]

use pyo3::prelude::*;

/// Base class for resource contents in Python bindings.
#[pyclass(subclass)]
#[derive(Clone)]
pub struct PyResourceContents;

/// Text resource contents for use in Python bindings.
#[pyclass(extends=PyResourceContents)]
#[derive(Clone)]
pub struct PyTextResourceContents {
    /// URI of the resource.
    #[pyo3(get, set)]
    pub uri: String,
    /// Text content of the resource.
    #[pyo3(get, set)]
    pub text: String,
    /// Optional MIME type of the resource.
    #[pyo3(get, set)]
    pub mime_type: Option<String>,
}

#[pymethods]
impl PyTextResourceContents {
    /// Creates a new `PyTextResourceContents`.
    ///
    /// # Arguments
    /// * `uri` - URI of the resource.
    /// * `text` - Text content.
    /// * `mime_type` - Optional MIME type.
    ///
    /// # Examples
    /// ```python
    /// text_resource = PyTextResourceContents(uri='file.txt', text='Hello', mime_type='text/plain')
    /// ```
    #[new]
    #[pyo3(signature = (uri, text, mime_type=None))]
    pub fn new(uri: String, text: String, mime_type: Option<String>) -> (Self, PyResourceContents) {
        (Self { uri, text, mime_type }, PyResourceContents)
    }
}

/// Blob resource contents for use in Python bindings.
#[pyclass(extends=PyResourceContents)]
#[derive(Clone)]
pub struct PyBlobResourceContents {
    /// URI of the blob resource.
    #[pyo3(get, set)]
    pub uri: String,
    /// Blob content as a base64-encoded string.
    #[pyo3(get, set)]
    pub blob: String,
    /// Optional MIME type of the blob.
    #[pyo3(get, set)]
    pub mime_type: Option<String>,
}

#[pymethods]
impl PyBlobResourceContents {
    /// Creates a new `PyBlobResourceContents`.
    ///
    /// # Arguments
    /// * `uri` - URI of the blob resource.
    /// * `blob` - Blob content as a base64-encoded string.
    /// * `mime_type` - Optional MIME type.
    ///
    /// # Examples
    /// ```python
    /// blob_resource = PyBlobResourceContents(uri='file.bin', blob='...', mime_type='application/octet-stream')
    /// ```
    #[new]
    #[pyo3(signature = (uri, blob, mime_type=None))]
    pub fn new(uri: String, blob: String, mime_type: Option<String>) -> (Self, PyResourceContents) {
        (Self { uri, blob, mime_type }, PyResourceContents)
    }
}
