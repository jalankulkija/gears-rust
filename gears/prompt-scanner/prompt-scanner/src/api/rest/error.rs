//! REST error mapping: `DomainError` -> `CanonicalError` (AIP-193 / RFC-9457
//! wire format), per ADR-0005 (`toolkit-canonical-errors`).
//!
//! Note: `docs/toolkit_unified_system/05_errors_rfc9457.md` describes a
//! simpler `Problem::builder()` mapping, but the actual codebase (all gears
//! surveyed, including the canonical `users-info` example) has moved to the
//! `CanonicalError` / `#[resource_error]` catalog pattern instead — grepping
//! for `Problem::builder` across `gears/` and `examples/` turns up zero
//! hits, vs. dozens of `#[resource_error(gts_id!(...))]` call sites. This
//! gear follows the pattern that actually compiles/lints cleanly in this
//! repo today.

use toolkit::api::canonical_prelude::*;

use crate::domain::error::DomainError;

#[resource_error(gts_id!("cf.core.prompt_scanner.scan_record.v1~"))]
struct ScanRecordResourceError;

#[resource_error(gts_id!("cf.core.prompt_scanner.tenant_policy.v1~"))]
struct TenantPolicyResourceError;

impl From<DomainError> for CanonicalError {
    #[allow(clippy::cognitive_complexity)]
    fn from(e: DomainError) -> Self {
        let ce = match &e {
            DomainError::VerdictNotFound { trace_id } => ScanRecordResourceError::not_found(
                format!("scan verdict with trace_id {trace_id} was not found"),
            )
            .with_resource(trace_id.to_string())
            .create(),

            DomainError::JobNotFound { job_id } => ScanRecordResourceError::not_found(format!(
                "scan job with job_id {job_id} was not found"
            ))
            .with_resource(job_id.to_string())
            .create(),

            DomainError::PolicyNotFound { tenant_id } => TenantPolicyResourceError::not_found(
                format!("no policy configured for tenant {tenant_id}"),
            )
            .with_resource(tenant_id.to_string())
            .create(),

            DomainError::Validation { field, message } => {
                ScanRecordResourceError::invalid_argument()
                    .with_field_violation(field, message, "VALIDATION")
                    .create()
            }

            DomainError::Forbidden => ScanRecordResourceError::permission_denied()
                .with_reason("ACCESS_DENIED")
                .create(),

            DomainError::BackendTimeout { backend } => ScanRecordResourceError::invalid_argument()
                .with_field_violation("backend", format!("'{backend}' timed out"), "TIMEOUT")
                .create(),

            DomainError::BackendUnavailable { backend, reason }
            | DomainError::RequiredBackendFailed { backend, reason } => {
                tracing::warn!(backend = %backend, reason = %reason, "backend unavailable");
                CanonicalError::internal(format!("backend '{backend}' unavailable: {reason}"))
                    .create()
            }

            // Ours, not the caller's: 503. The reason may name hosts or carry
            // upstream error text, so it goes to the log, not the wire.
            DomainError::PolicyUnavailable { tenant_id, reason } => {
                tracing::warn!(tenant_id = %tenant_id, reason = %reason, "tenant policy unavailable");
                CanonicalError::service_unavailable()
                    .with_detail(format!(
                        "policy for tenant {tenant_id} is temporarily unavailable"
                    ))
                    .create()
            }

            DomainError::AttachmentParsingFailed { reason } => {
                ScanRecordResourceError::invalid_argument()
                    .with_field_violation("attachment", reason, "ATTACHMENT_PARSE_FAILED")
                    .create()
            }

            DomainError::Database { .. } => {
                tracing::error!(error = ?e, "Database error occurred");
                CanonicalError::internal("An internal database error occurred").create()
            }

            DomainError::Internal(_) => {
                tracing::error!(error = ?e, "Internal error occurred");
                CanonicalError::internal("An internal error occurred").create()
            }
        };

        if let Some(diag) = ce.diagnostic() {
            tracing::debug!(diagnostic = %diag, "Canonical error diagnostic");
        }

        ce
    }
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod error_tests;
