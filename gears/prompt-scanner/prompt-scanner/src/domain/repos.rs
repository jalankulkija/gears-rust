//! Repository traits for the prompt-scanner gear's persisted aggregates.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use prompt_scanner_sdk::{ContentKind, Finding, ScanVerdict, VerdictAction};
use toolkit_db::secure::DBRunner;
use toolkit_macros::domain_model;
use toolkit_odata::{ODataQuery, Page};
use toolkit_security::AccessScope;
use uuid::Uuid;

use crate::domain::error::DomainError;

/// A persisted scan audit-trail entry.
///
/// Carries fields beyond [`ScanVerdict`] (`connector_id`, `content_kind`)
/// that the SDK's public verdict model deliberately omits (connectors don't
/// need them echoed back), but which the `scan_record` audit trail requires
/// per `docs/DESIGN.md`.
#[domain_model]
#[derive(Debug, Clone, PartialEq)]
pub struct ScanRecord {
    pub trace_id: Uuid,
    pub tenant_id: Uuid,
    pub connector_id: String,
    pub content_kind: ContentKind,
    pub action: VerdictAction,
    pub findings: Vec<Finding>,
    pub redacted_payload: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<&ScanRecord> for ScanVerdict {
    fn from(r: &ScanRecord) -> Self {
        Self {
            trace_id: r.trace_id,
            tenant_id: r.tenant_id,
            action: r.action,
            findings: r.findings.clone(),
            redacted_payload: r.redacted_payload.clone(),
            created_at: r.created_at,
        }
    }
}

impl From<ScanRecord> for ScanVerdict {
    fn from(r: ScanRecord) -> Self {
        Self {
            trace_id: r.trace_id,
            tenant_id: r.tenant_id,
            action: r.action,
            findings: r.findings,
            redacted_payload: r.redacted_payload,
            created_at: r.created_at,
        }
    }
}

/// Repository for the `scan_record` audit trail.
///
/// All methods accept `runner: &C` (a secure DB runner, `&SecureConn` or
/// `&SecureTx`) and `scope: &AccessScope` prepared by the service layer.
#[async_trait]
pub trait ScanRecordRepository: Send + Sync {
    /// Find a scan record by trace id within the given security scope.
    async fn get<C: DBRunner>(
        &self,
        runner: &C,
        scope: &AccessScope,
        trace_id: Uuid,
    ) -> Result<Option<ScanRecord>, DomainError>;

    /// List scan records with `OData` filtering/pagination.
    async fn list_page<C: DBRunner>(
        &self,
        runner: &C,
        scope: &AccessScope,
        query: &ODataQuery,
    ) -> Result<Page<ScanRecord>, DomainError>;

    /// Persist a new scan record.
    async fn create<C: DBRunner>(
        &self,
        runner: &C,
        scope: &AccessScope,
        record: ScanRecord,
    ) -> Result<ScanRecord, DomainError>;

    /// Update the `action`/`findings` of an existing scan record in place.
    ///
    /// Used to merge a deferred backend's finding into a scan record that
    /// was already persisted with the synchronous backends' findings — see
    /// `domain::service`'s "Async deferral" docs.
    async fn update_findings<C: DBRunner>(
        &self,
        runner: &C,
        scope: &AccessScope,
        trace_id: Uuid,
        action: VerdictAction,
        findings: Vec<Finding>,
    ) -> Result<(), DomainError>;
}
