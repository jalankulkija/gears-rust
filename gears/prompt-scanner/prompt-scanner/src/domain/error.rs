//! Domain error type for the prompt-scanner gear.

use prompt_scanner_sdk::PromptScannerError;
use thiserror::Error;
use toolkit_db::DbError;
use toolkit_db::secure::{InfraError, ScopeError};
use toolkit_macros::domain_model;
use uuid::Uuid;

/// Domain-specific errors.
#[domain_model]
#[derive(Error, Debug, Clone)]
pub enum DomainError {
    #[error("scan verdict not found: {trace_id}")]
    VerdictNotFound { trace_id: Uuid },

    #[error("scan job not found: {job_id}")]
    JobNotFound { job_id: Uuid },

    #[error("tenant policy not found for tenant {tenant_id}")]
    PolicyNotFound { tenant_id: Uuid },

    #[error("validation error: {field}: {message}")]
    Validation { field: String, message: String },

    #[error("access denied")]
    Forbidden,

    #[error("backend '{backend}' timed out")]
    BackendTimeout { backend: String },

    #[error("backend '{backend}' unavailable: {reason}")]
    BackendUnavailable { backend: String, reason: String },

    #[error("required backend '{backend}' failed: {reason}")]
    RequiredBackendFailed { backend: String, reason: String },

    #[error("policy for tenant {tenant_id} unavailable: {reason}")]
    PolicyUnavailable { tenant_id: Uuid, reason: String },

    #[error("attachment parsing failed: {reason}")]
    AttachmentParsingFailed { reason: String },

    #[error("database error: {message}")]
    Database { message: String },

    #[error("internal error: {0}")]
    Internal(String),
}

impl DomainError {
    #[must_use]
    pub fn verdict_not_found(trace_id: Uuid) -> Self {
        Self::VerdictNotFound { trace_id }
    }

    #[must_use]
    pub fn job_not_found(job_id: Uuid) -> Self {
        Self::JobNotFound { job_id }
    }

    #[must_use]
    pub fn policy_not_found(tenant_id: Uuid) -> Self {
        Self::PolicyNotFound { tenant_id }
    }

    pub fn validation(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Validation {
            field: field.into(),
            message: message.into(),
        }
    }

    pub fn database(message: impl Into<String>) -> Self {
        Self::Database {
            message: message.into(),
        }
    }

    /// Convert an infrastructure error into a domain database error.
    #[must_use]
    #[allow(clippy::needless_pass_by_value)]
    pub fn database_infra(e: InfraError) -> Self {
        Self::database(e.to_string())
    }
}

// These `From` impls carry only a `String`, dropping the original error as
// `.source()`. Extend `Database`/`Internal` to hold a boxed source if
// `.source()` fidelity becomes necessary, then remove these allows.
#[allow(unknown_lints, de1302_error_from_to_string)]
impl From<DbError> for DomainError {
    fn from(e: DbError) -> Self {
        Self::database(e.to_string())
    }
}

#[allow(unknown_lints, de1302_error_from_to_string)]
impl From<ScopeError> for DomainError {
    fn from(e: ScopeError) -> Self {
        Self::database(e.to_string())
    }
}

impl From<authz_resolver_sdk::EnforcerError> for DomainError {
    fn from(e: authz_resolver_sdk::EnforcerError) -> Self {
        tracing::error!(error = %e, "AuthZ scope resolution failed");
        match e {
            authz_resolver_sdk::EnforcerError::Denied { .. }
            | authz_resolver_sdk::EnforcerError::CompileFailed(_) => Self::Forbidden,
            authz_resolver_sdk::EnforcerError::EvaluationFailed(_) => {
                Self::Internal("authz evaluation failed".to_owned())
            }
        }
    }
}

#[allow(unknown_lints, de1302_error_from_to_string)]
impl From<toolkit_canonical_errors::CanonicalError> for DomainError {
    fn from(e: toolkit_canonical_errors::CanonicalError) -> Self {
        Self::Internal(e.diagnostic().map_or_else(|| e.to_string(), str::to_owned))
    }
}

#[allow(unknown_lints, de1302_error_from_to_string)]
impl From<toolkit::client_hub::ClientHubError> for DomainError {
    fn from(e: toolkit::client_hub::ClientHubError) -> Self {
        Self::Internal(e.to_string())
    }
}

#[allow(unknown_lints, de1302_error_from_to_string)]
impl From<file_parser_sdk::FileParserClientError> for DomainError {
    fn from(e: file_parser_sdk::FileParserClientError) -> Self {
        Self::AttachmentParsingFailed {
            reason: e.to_string(),
        }
    }
}

/// Domain → SDK error conversion.
impl From<DomainError> for PromptScannerError {
    fn from(e: DomainError) -> Self {
        match e {
            DomainError::VerdictNotFound { trace_id } => Self::verdict_not_found(trace_id),
            DomainError::JobNotFound { job_id } => Self::job_not_found(job_id),
            DomainError::PolicyNotFound { tenant_id } => Self::policy_not_found(tenant_id),
            DomainError::Validation { field, message } => {
                Self::validation(format!("{field}: {message}"))
            }
            DomainError::Forbidden => Self::Forbidden,
            DomainError::BackendTimeout { backend } => Self::BackendTimeout { backend },
            DomainError::BackendUnavailable { backend, reason } => {
                Self::BackendUnavailable { backend, reason }
            }
            DomainError::RequiredBackendFailed { backend, reason } => {
                Self::RequiredBackendFailed { backend, reason }
            }
            DomainError::PolicyUnavailable { tenant_id, reason } => {
                Self::PolicyUnavailable { tenant_id, reason }
            }
            DomainError::AttachmentParsingFailed { reason } => {
                Self::AttachmentParsingFailed { reason }
            }
            DomainError::Database { message } | DomainError::Internal(message) => {
                Self::internal(message)
            }
        }
    }
}

/// SDK → domain error conversion — the reverse of the `From<DomainError> for
/// PromptScannerError` impl above. Needed because `Service` now calls
/// through the transport-agnostic `PolicyProvider` trait (which returns
/// `PromptScannerError`) rather than a domain-internal repository trait.
#[allow(unknown_lints, de1302_error_from_to_string)]
impl From<PromptScannerError> for DomainError {
    fn from(e: PromptScannerError) -> Self {
        match e {
            PromptScannerError::VerdictNotFound { trace_id } => Self::VerdictNotFound { trace_id },
            PromptScannerError::JobNotFound { job_id } => Self::JobNotFound { job_id },
            PromptScannerError::PolicyNotFound { tenant_id } => Self::PolicyNotFound { tenant_id },
            PromptScannerError::Validation { message } => Self::Validation {
                field: "policy".to_owned(),
                message,
            },
            PromptScannerError::Forbidden => Self::Forbidden,
            PromptScannerError::BackendTimeout { backend } => Self::BackendTimeout { backend },
            PromptScannerError::BackendUnavailable { backend, reason } => {
                Self::BackendUnavailable { backend, reason }
            }
            PromptScannerError::RequiredBackendFailed { backend, reason } => {
                Self::RequiredBackendFailed { backend, reason }
            }
            PromptScannerError::PolicyUnavailable { tenant_id, reason } => {
                Self::PolicyUnavailable { tenant_id, reason }
            }
            PromptScannerError::AttachmentParsingFailed { reason } => {
                Self::AttachmentParsingFailed { reason }
            }
            PromptScannerError::Internal(message) => Self::Internal(message),
        }
    }
}
