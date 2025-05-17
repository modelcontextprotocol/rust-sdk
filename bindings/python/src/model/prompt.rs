//! Python bindings for prompt-related types.
//!
//! This module defines Rust representations of prompts, prompt arguments, and prompt messages for use in Python bindings.
//!
//! # Examples
//!
//! ```python
//! from rmcp_python import PyPrompt, PyPromptArgument, PyPromptMessage
//! prompt = PyPrompt(name='example', description='A prompt', arguments=[])
//! ```
#![allow(non_local_definitions)]

use pyo3::prelude::*;

/// Represents a prompt for use in Python bindings.
#[pyclass]
#[derive(Clone)]
pub struct PyPrompt {
    /// Name of the prompt.
    #[pyo3(get, set)]
    pub name: String,
    /// Optional description of the prompt.
    #[pyo3(get, set)]
    pub description: Option<String>,
    /// Optional list of arguments for the prompt.
    #[pyo3(get, set)]
    pub arguments: Option<Vec<Py<PyPromptArgument>>>,

}

#[pymethods]
impl PyPrompt {
    /// Creates a new `PyPrompt`.
    ///
    /// # Arguments
    /// * `name` - Name of the prompt.
    /// * `description` - Optional description.
    /// * `arguments` - Optional list of prompt arguments.
    ///
    /// # Examples
    /// ```python
    /// prompt = PyPrompt(name='example', description='A prompt', arguments=[])
    /// ```
    #[new]
    pub fn new(name: String, description: Option<String>, arguments: Option<Vec<Py<PyPromptArgument>>>) -> Self {
        Self { name, description, arguments }
    }
}

/// Represents an argument to a prompt, for use in Python bindings.
#[pyclass]
#[derive(Clone)]
pub struct PyPromptArgument {
    /// Name of the argument.
    #[pyo3(get, set)]
    pub name: String,
    /// Optional description of the argument.
    #[pyo3(get, set)]
    pub description: Option<String>,
    /// Whether the argument is required.
    #[pyo3(get, set)]
    pub required: Option<bool>,
}

#[pymethods]
impl PyPromptArgument {
    /// Creates a new `PyPromptArgument`.
    ///
    /// # Arguments
    /// * `name` - Name of the argument.
    /// * `description` - Optional description.
    /// * `required` - Whether the argument is required.
    ///
    /// # Examples
    /// ```python
    /// arg = PyPromptArgument(name='input', description='User input', required=True)
    /// ```
    #[new]
    pub fn new(name: String, description: Option<String>, required: Option<bool>) -> Self {
        Self { name, description, required }
    }
}

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
