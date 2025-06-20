//! Python bindings for a single prompt message.
//!
//! This module defines the `PyPromptMessage` struct for representing prompt messages in Python.
//!
//! # Examples
//!
//! ```python
//! from rmcp_python import PyPromptMessage
//! msg = PyPromptMessage(role='user', content='Hello!')
//! ```
use pyo3::prelude::*;

/// Represents a prompt message for use in Python bindings.
#[pyclass]
#[derive(Clone)]
pub struct PyPromptMessage {
    /// Role associated with the message.
    #[pyo3(get, set)]
    pub role: String,
    /// Content of the message.
    #[pyo3(get, set)]
    pub content: String,
}

#[pymethods]
impl PyPromptMessage {
    /// Creates a new `PyPromptMessage`.
    ///
    /// # Arguments
    /// * `role` - Role associated with the message.
    /// * `content` - Content of the message.
    ///
    /// # Examples
    /// ```python
    /// msg = PyPromptMessage(role='user', content='Hello!')
    /// ```
    #[new]
    pub fn new(role: String, content: String) -> Self {
        Self { role, content }
    }
}
