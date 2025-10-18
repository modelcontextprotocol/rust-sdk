use std::{borrow::Cow, fmt::Display};

use crate::ServiceError;
pub use crate::model::ErrorData;
#[deprecated(
    note = "Use `rmcp::ErrorData` instead, `rmcp::ErrorData` could become `RmcpError` in the future."
)]
pub type Error = ErrorData;
impl Display for ErrorData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.0, self.message)?;
        if let Some(data) = &self.data {
            write!(f, "({})", data)?;
        }
        Ok(())
    }
}

impl std::error::Error for ErrorData {}

/// This is an unified error type for the errors could be returned by the service.
#[derive(Debug, thiserror::Error)]
pub enum RmcpError {
    #[error("Service error: {0}")]
    Service(#[from] ServiceError),
    #[cfg(feature = "client")]
    #[error("Client initialization error: {0}")]
    ClientInitialize(#[from] crate::service::ClientInitializeError),
    #[cfg(feature = "server")]
    #[error("Server initialization error: {0}")]
    ServerInitialize(#[from] crate::service::ServerInitializeError),
    #[error("Runtime error: {0}")]
    Runtime(#[from] tokio::task::JoinError),
    #[error("Transport creation error: {error}")]
    // TODO: Maybe we can introduce something like `TryIntoTransport` to auto wrap transport type,
    // but it could be an breaking change, so we could do it in the future.
    TransportCreation {
        into_transport_type_name: Cow<'static, str>,
        into_transport_type_id: std::any::TypeId,
        #[source]
        error: Box<dyn std::error::Error + Send + Sync>,
    },
    // and cancellation shouldn't be an error?
}

impl RmcpError {
    pub fn transport_creation<T: 'static>(
        error: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        RmcpError::TransportCreation {
            into_transport_type_id: std::any::TypeId::of::<T>(),
            into_transport_type_name: std::any::type_name::<T>().into(),
            error: error.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;
    use crate::model::{ErrorCode, ErrorData};

    #[test]
    fn test_error_data_display_without_data() {
        let error = ErrorData {
            code: ErrorCode(-32600),
            message: "Invalid Request".into(),
            data: None,
        };
        assert_eq!(format!("{}", error), "-32600: Invalid Request");
    }

    #[test]
    fn test_error_data_display_with_data() {
        let error = ErrorData {
            code: ErrorCode(-32600),
            message: "Invalid Request".into(),
            data: Some(serde_json::json!({"detail": "missing field"})),
        };
        assert_eq!(
            format!("{}", error),
            "-32600: Invalid Request({\"detail\":\"missing field\"})"
        );
    }

    #[test]
    fn test_rmcp_error_transport_creation() {
        struct DummyTransport;
        let io_error = io::Error::other("connection failed");
        let error = RmcpError::transport_creation::<DummyTransport>(io_error);

        match error {
            RmcpError::TransportCreation {
                into_transport_type_name,
                into_transport_type_id,
                ..
            } => {
                assert!(into_transport_type_name.contains("DummyTransport"));
                assert_eq!(
                    into_transport_type_id,
                    std::any::TypeId::of::<DummyTransport>()
                );
            }
            _ => panic!("Expected TransportCreation variant"),
        }
    }

    #[test]
    fn test_rmcp_error_display() {
        struct DummyTransport;
        let io_error = io::Error::other("connection failed");
        let error = RmcpError::transport_creation::<DummyTransport>(io_error);
        let display = format!("{}", error);
        assert!(display.contains("Transport creation error"));
    }

    #[test]
    fn test_error_data_is_std_error() {
        let error = ErrorData {
            code: ErrorCode(-32600),
            message: "Invalid Request".into(),
            data: None,
        };
        let _: &dyn std::error::Error = &error;
    }
}
