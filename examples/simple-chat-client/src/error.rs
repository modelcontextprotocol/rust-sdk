use std::fmt;

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct McpError {
    pub message: String,
}

impl fmt::Display for McpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for McpError {}

impl McpError {
    pub fn new(message: impl ToString) -> Self {
        Self {
            message: message.to_string(),
        }
    }
}
