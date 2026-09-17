//! Deterministic regex/validator scanning: runs the configured pattern set
//! over the payload and reports *every* match as its own `Finding` — a
//! message can legitimately contain more than one distinct piece of PII or
//! secret material, and under-reporting to "just the worst one" would mean
//! silently missing everything else in the same payload (see
//! `prompt_scanner_sdk::BackendOutcome::Findings`).
//!
//! No `onnx-runner` dependency and no async I/O: Tier-0 detection per
//! `gears/onnx-runner/docs/DESIGN.md` — near-zero cost, language-invariant
//! recall that no ML model in this platform replaces.
//!
//! What is detected is configuration, not code: the pattern set arrives
//! already compiled from `crate::config` (see `crate::domain::pattern_set`).

use std::sync::Arc;

use async_trait::async_trait;
use prompt_scanner_sdk::{
    BackendOutcome, Finding, PromptScannerError, ScanInput, ScannerBackendClient, Severity, Span,
    VerdictAction,
};
use toolkit_macros::domain_model;
use toolkit_security::SecurityContext;

use crate::domain::pattern_set::{Match, PatternSet};

/// Severity corresponding to a suggested action — kept consistent across
/// backends so a tenant's thresholds behave predictably regardless of which
/// backend produced a finding.
const fn severity_for_action(action: VerdictAction) -> Severity {
    match action {
        VerdictAction::Block => Severity::Critical,
        VerdictAction::Redact => Severity::High,
        VerdictAction::Warn => Severity::Medium,
        _ => Severity::Low,
    }
}

fn match_to_finding(m: Match) -> Finding {
    Finding {
        detector: String::new(), // prompt-scanner fills this in from the registered vendor
        category: m.category,
        severity: severity_for_action(m.suggested_action),
        confidence: m.tier.confidence(),
        span: Some(Span {
            start: m.span.0,
            end: m.span.1,
        }),
        suggested_action: m.suggested_action,
    }
}

#[domain_model]
pub struct Service {
    patterns: Arc<PatternSet>,
}

impl Service {
    #[must_use]
    pub const fn new(patterns: Arc<PatternSet>) -> Self {
        Self { patterns }
    }
}

#[async_trait]
impl ScannerBackendClient for Service {
    async fn detect(
        &self,
        _ctx: &SecurityContext,
        input: ScanInput,
    ) -> Result<BackendOutcome, PromptScannerError> {
        let findings = self
            .patterns
            .scan(&input.payload)
            .into_iter()
            .map(match_to_finding)
            .collect();

        Ok(BackendOutcome::Findings(findings))
    }
}
