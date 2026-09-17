//! Trivial "always allow" backend: inspects nothing and reports nothing.
//!
//! It exists so `prompt-scanner`'s fan-out resolution logic
//! (`domain::service::Service::resolve_backends` and `run_backends`) has a
//! real, in-repo plugin instance to resolve and call in integration tests.
//! It is a test double, not a detector — `regex-scanner` and `onnx-scanner`
//! are the real backends.
//!
//! # Why it reports an *empty* finding set
//!
//! A backend with nothing to say must return `Findings(vec![])`, never a
//! placeholder finding. `domain::policy::aggregate_verdict` applies the
//! tenant's `default_action` only when **no** finding survives threshold
//! filtering, so a single meaningless zero-confidence `Allow` is enough to
//! consume that fallback and silently replace a tenant's `default_action:
//! Block` with `Allow`. `default_action` is a fallback, not a floor, and
//! cannot be made one (`Block` is the maximum of the ordering, so a floor
//! would block every request). The invariant therefore lives here, and is
//! pinned by `policy::tests::any_finding_suppresses_default_action_even_a_zero_confidence_allow`.

use async_trait::async_trait;
use prompt_scanner_sdk::{BackendOutcome, PromptScannerError, ScanInput, ScannerBackendClient};
use toolkit_macros::domain_model;
use toolkit_security::SecurityContext;

#[domain_model]
pub struct Service;

#[async_trait]
impl ScannerBackendClient for Service {
    async fn detect(
        &self,
        _ctx: &SecurityContext,
        _input: ScanInput,
    ) -> Result<BackendOutcome, PromptScannerError> {
        Ok(BackendOutcome::Findings(Vec::new()))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::collections::HashMap;

    use prompt_scanner_sdk::ContentKind;
    use uuid::Uuid;

    use super::*;

    fn ctx() -> SecurityContext {
        SecurityContext::builder()
            .subject_id(Uuid::new_v4())
            .subject_tenant_id(Uuid::new_v4())
            .build()
            .expect("valid SecurityContext")
    }

    fn input() -> ScanInput {
        ScanInput {
            tenant_id: Uuid::nil(),
            content_kind: ContentKind::Prompt,
            payload: "anything at all".to_owned(),
            metadata: HashMap::default(),
        }
    }

    /// Reporting nothing must mean an empty finding set, not a placeholder
    /// finding — see the module docs for what a placeholder would break.
    #[tokio::test]
    async fn reports_an_empty_finding_set_rather_than_a_placeholder() {
        let outcome = Service
            .detect(&ctx(), input())
            .await
            .expect("always-allow never errors");

        let BackendOutcome::Findings(findings) = outcome else {
            panic!("expected a Findings outcome");
        };
        assert!(
            findings.is_empty(),
            "a backend that inspects nothing must report nothing, got: {findings:?}"
        );
    }
}
