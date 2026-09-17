//! Local (in-process) client for the prompt-scanner gear.

use std::sync::Arc;

use async_trait::async_trait;
use prompt_scanner_sdk::{
    PromptScannerClientV1, PromptScannerError, ScanJobHandle, ScanRequest, ScanVerdict,
    TenantPolicy,
};
use toolkit_macros::domain_model;
use toolkit_odata::{ODataQuery, Page};
use toolkit_security::SecurityContext;
use uuid::Uuid;

use super::repos::ScanRecordRepository;
use super::service::Service;

/// Local client wrapping the prompt-scanner domain service.
///
/// Registered in `ClientHub` by the gear during `init()`.
#[domain_model]
pub struct PromptScannerLocalClient<SR>
where
    SR: ScanRecordRepository + 'static,
{
    svc: Arc<Service<SR>>,
}

impl<SR> PromptScannerLocalClient<SR>
where
    SR: ScanRecordRepository + 'static,
{
    #[must_use]
    pub fn new(svc: Arc<Service<SR>>) -> Self {
        Self { svc }
    }
}

fn log_and_convert(op: &str, e: super::DomainError) -> PromptScannerError {
    tracing::error!(operation = op, error = ?e, "prompt-scanner call failed");
    e.into()
}

#[async_trait]
impl<SR> PromptScannerClientV1 for PromptScannerLocalClient<SR>
where
    SR: ScanRecordRepository + 'static,
{
    async fn scan(
        &self,
        ctx: &SecurityContext,
        req: ScanRequest,
    ) -> Result<ScanVerdict, PromptScannerError> {
        self.svc
            .scan(ctx, req)
            .await
            .map_err(|e| log_and_convert("scan", e))
    }

    async fn scan_async(
        &self,
        ctx: &SecurityContext,
        req: ScanRequest,
    ) -> Result<ScanJobHandle, PromptScannerError> {
        self.svc
            .scan_async(ctx, req)
            .await
            .map_err(|e| log_and_convert("scan_async", e))
    }

    async fn get_verdict(
        &self,
        ctx: &SecurityContext,
        trace_id: Uuid,
    ) -> Result<ScanVerdict, PromptScannerError> {
        self.svc
            .get_verdict(ctx, trace_id)
            .await
            .map_err(|e| log_and_convert("get_verdict", e))
    }

    async fn list_scans(
        &self,
        ctx: &SecurityContext,
        query: &ODataQuery,
    ) -> Result<Page<ScanVerdict>, PromptScannerError> {
        self.svc
            .list_scans(ctx, query)
            .await
            .map_err(|e| log_and_convert("list_scans", e))
    }

    async fn get_policy(&self, ctx: &SecurityContext) -> Result<TenantPolicy, PromptScannerError> {
        self.svc
            .get_policy(ctx)
            .await
            .map_err(|e| log_and_convert("get_policy", e))
    }

    async fn put_policy(
        &self,
        ctx: &SecurityContext,
        policy: TenantPolicy,
    ) -> Result<TenantPolicy, PromptScannerError> {
        self.svc
            .put_policy(ctx, policy)
            .await
            .map_err(|e| log_and_convert("put_policy", e))
    }
}
