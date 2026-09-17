//! Public API trait for the prompt-scanner gear.
//!
//! Connectors (`llm-gateway`'s `scan-hook-plugin`, endpoint agents, MCP
//! proxies, ...) call this trait to submit content for scanning and to
//! manage per-tenant backend policy.

use async_trait::async_trait;
use toolkit_odata::{ODataQuery, Page};
use toolkit_security::SecurityContext;
use uuid::Uuid;

use crate::errors::PromptScannerError;
use crate::models::{ScanJobHandle, ScanRequest, ScanVerdict, TenantPolicy};

/// Public API trait for the prompt-scanner gear.
///
/// Obtain via `ClientHub`:
/// ```ignore
/// let client = hub.get::<dyn PromptScannerClientV1>()?;
/// let verdict = client.scan(&ctx, req).await?;
/// ```
#[async_trait]
pub trait PromptScannerClientV1: Send + Sync {
    /// Synchronously scan a piece of content and return the aggregated
    /// verdict. This is the main connector ingestion path.
    ///
    /// # Errors
    ///
    /// - `Validation` if the request is malformed.
    /// - `RequiredBackendFailed` if a tenant-required backend errored or
    ///   timed out.
    /// - `PolicyUnavailable` if the tenant's policy could not be loaded.
    async fn scan(
        &self,
        ctx: &SecurityContext,
        req: ScanRequest,
    ) -> Result<ScanVerdict, PromptScannerError>;

    /// Queue a scan for asynchronous processing (e.g. large uploads) and
    /// return a job handle. Poll `get_verdict` with the returned `trace_id`
    /// once the job completes.
    ///
    /// # Errors
    ///
    /// - `Validation` if the request is malformed.
    async fn scan_async(
        &self,
        ctx: &SecurityContext,
        req: ScanRequest,
    ) -> Result<ScanJobHandle, PromptScannerError>;

    /// Fetch a previously recorded verdict by trace id (traceability /
    /// audit lookups).
    ///
    /// # Errors
    ///
    /// - `VerdictNotFound` if no verdict exists for the given trace id
    ///   (including if the async job that would produce it hasn't finished).
    async fn get_verdict(
        &self,
        ctx: &SecurityContext,
        trace_id: Uuid,
    ) -> Result<ScanVerdict, PromptScannerError>;

    /// `OData`-filterable audit search over recorded verdicts, scoped to
    /// whatever the caller's `SecurityContext` is authorized to see. Used
    /// by admin/demo UIs (e.g. the audit log panel in
    /// `demo-policy-console`) that want to read the audit trail without a
    /// direct REST hop.
    ///
    /// # Errors
    ///
    /// Returns [`PromptScannerError`] on authorization failure or an
    /// invalid `OData` query.
    async fn list_scans(
        &self,
        ctx: &SecurityContext,
        query: &ODataQuery,
    ) -> Result<Page<ScanVerdict>, PromptScannerError>;

    /// Get the calling tenant's backend policy.
    ///
    /// # Errors
    ///
    /// - `PolicyNotFound` if the tenant has never configured a policy (some
    ///   implementations may instead return a permissive default — see the
    ///   gear's own docs).
    async fn get_policy(&self, ctx: &SecurityContext) -> Result<TenantPolicy, PromptScannerError>;

    /// Update the calling tenant's backend policy.
    ///
    /// # Errors
    ///
    /// - `Validation` if the policy references unknown backends or invalid
    ///   thresholds.
    async fn put_policy(
        &self,
        ctx: &SecurityContext,
        policy: TenantPolicy,
    ) -> Result<TenantPolicy, PromptScannerError>;
}
