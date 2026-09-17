//! Transport-agnostic error type for the prompt-scanner gear.

use thiserror::Error;
use uuid::Uuid;

/// Errors that can occur when using the prompt-scanner API.
#[derive(Debug, Clone, Error)]
pub enum PromptScannerError {
    /// No verdict recorded for the given trace id.
    #[error("scan verdict not found: {trace_id}")]
    VerdictNotFound {
        /// The trace id that was not found.
        trace_id: Uuid,
    },

    /// No async job found for the given job id.
    #[error("scan job not found: {job_id}")]
    JobNotFound {
        /// The job id that was not found.
        job_id: Uuid,
    },

    /// No policy has been configured for the tenant.
    #[error("tenant policy not found for tenant {tenant_id}")]
    PolicyNotFound {
        /// The tenant id with no configured policy.
        tenant_id: Uuid,
    },

    /// The request failed validation.
    #[error("validation error: {message}")]
    Validation {
        /// Human-readable description of the validation failure.
        message: String,
    },

    /// The caller is not authorized to perform the requested operation.
    #[error("access denied")]
    Forbidden,

    /// A scanner backend did not respond within the configured timeout.
    #[error("backend '{backend}' timed out")]
    BackendTimeout {
        /// Vendor identifier of the backend.
        backend: String,
    },

    /// A scanner backend is not currently resolvable/available.
    #[error("backend '{backend}' unavailable: {reason}")]
    BackendUnavailable {
        /// Vendor identifier of the backend.
        backend: String,
        /// Human-readable reason.
        reason: String,
    },

    /// A required backend (per tenant policy) failed or timed out.
    #[error("required backend '{backend}' failed: {reason}")]
    RequiredBackendFailed {
        /// Vendor identifier of the required backend.
        backend: String,
        /// Human-readable reason.
        reason: String,
    },

    /// The tenant's policy could not be loaded: its policy provider's source
    /// is unreachable and the provider has nothing to serve in its place.
    /// The caller is not at fault; retrying later may succeed.
    #[error("policy for tenant {tenant_id} unavailable: {reason}")]
    PolicyUnavailable {
        /// The tenant whose policy could not be loaded.
        tenant_id: Uuid,
        /// Human-readable reason, for logs only: it may name hosts or carry
        /// upstream error text, so it never reaches the wire.
        reason: String,
    },

    /// Extracting text from a `ContentKind::Upload` attachment failed (e.g.
    /// unsupported file type, corrupt file, or `file-parser` itself was
    /// unreachable).
    #[error("attachment parsing failed: {reason}")]
    AttachmentParsingFailed {
        /// Human-readable reason.
        reason: String,
    },

    /// An unexpected internal error occurred.
    #[error("internal error: {0}")]
    Internal(String),
}

impl PromptScannerError {
    /// Construct a [`PromptScannerError::VerdictNotFound`].
    #[must_use]
    pub fn verdict_not_found(trace_id: Uuid) -> Self {
        Self::VerdictNotFound { trace_id }
    }

    /// Construct a [`PromptScannerError::JobNotFound`].
    #[must_use]
    pub fn job_not_found(job_id: Uuid) -> Self {
        Self::JobNotFound { job_id }
    }

    /// Construct a [`PromptScannerError::PolicyNotFound`].
    #[must_use]
    pub fn policy_not_found(tenant_id: Uuid) -> Self {
        Self::PolicyNotFound { tenant_id }
    }

    /// Construct a [`PromptScannerError::PolicyUnavailable`].
    pub fn policy_unavailable(tenant_id: Uuid, reason: impl Into<String>) -> Self {
        Self::PolicyUnavailable {
            tenant_id,
            reason: reason.into(),
        }
    }

    /// Construct a [`PromptScannerError::Validation`].
    pub fn validation(message: impl Into<String>) -> Self {
        Self::Validation {
            message: message.into(),
        }
    }

    /// Construct a [`PromptScannerError::Internal`].
    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }
}
