//! Model types for the Python bindings.
//!
//! This module exposes Rust model types (resources, prompts, tools, capabilities) to Python via pyo3 bindings.
//!
//! - `resource`: Resource contents (text/blob)
//! - `prompt`: Prompts, prompt arguments, and prompt messages
//! - `tool`: Tools and tool annotations (if enabled)
//! - `capabilities`: Client and roots capabilities

pub mod resource;
pub mod prompt;
pub mod tool;
pub mod capabilities;
