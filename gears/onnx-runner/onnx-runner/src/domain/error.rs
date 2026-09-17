use thiserror::Error;
use toolkit_macros::domain_model;

/// Domain-level errors for the `onnx-runner` gear.
#[domain_model]
#[derive(Error, Debug, Clone)]
pub enum DomainError {
    #[error("model contract mismatch for {model_id}: {message}")]
    ContractMismatch { model_id: String, message: String },

    #[error("checksum mismatch for {model_id}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        model_id: String,
        expected: String,
        actual: String,
    },

    #[error("failed to load model {model_id}: {message}")]
    LoadFailed { model_id: String, message: String },

    #[error("unknown model id: {0}")]
    UnknownModel(String),
}
