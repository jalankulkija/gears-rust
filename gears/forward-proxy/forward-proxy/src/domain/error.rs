use thiserror::Error;
use toolkit_macros::domain_model;

/// Domain-level errors for the `forward-proxy` gear.
#[domain_model]
#[derive(Error, Debug, Clone)]
pub enum DomainError {
    #[error("CA material unavailable: {message}")]
    CaUnavailable { message: String },

    #[error("failed to bind proxy listener on {addr}: {message}")]
    BindFailed { addr: String, message: String },
}

impl DomainError {
    pub fn ca_unavailable(message: impl Into<String>) -> Self {
        Self::CaUnavailable {
            message: message.into(),
        }
    }

    pub fn bind_failed(addr: impl Into<String>, message: impl Into<String>) -> Self {
        Self::BindFailed {
            addr: addr.into(),
            message: message.into(),
        }
    }
}
