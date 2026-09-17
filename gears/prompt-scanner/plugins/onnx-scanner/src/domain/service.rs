//! PII detection orchestration: word-count deferral, confidence-to-action
//! mapping, `Finding` construction. Model-specific detection itself lives
//! behind [`PiiBackend`] — see `domain::backend`'s docs for why, and
//! `domain::bio_backend`/`domain::gliner2` for the two implementations.
//!
//! # Deferral for oversized inputs
//!
//! If the payload's word count exceeds `defer_threshold_words`, `detect`
//! returns [`BackendOutcome::Defer`] immediately, before calling the
//! backend at all — cost here scales with token count, so a very long
//! payload risks blowing the fan-out's per-backend timeout (see
//! `prompt-scanner`'s `domain::service::Service::run_backends`).
//!
//! **Known scaffold-level limitation**: `prompt-scanner`'s background job
//! worker re-invokes this same `detect` method with the same payload when
//! draining a deferred entry — since word count hasn't changed, it defers
//! again, and `prompt-scanner` drops a repeat `Defer` rather than
//! re-queuing indefinitely (see `run_backends`' module docs there). So
//! today, a payload over the threshold effectively never gets a delayed
//! verdict — it's dropped after one deferred attempt (logged). Fixing this
//! needs either a way to signal "this call has no timeout, actually do the
//! work" to `detect`, or chunking cheap enough to always attempt inline;
//! neither is implemented in this scaffold. Tracked as follow-up, not
//! blocking v1: today's mitigation is a `defer_threshold_words` generous
//! enough that this path is rarely hit in practice.

use std::sync::Arc;

use async_trait::async_trait;
use prompt_scanner_sdk::{
    BackendOutcome, Finding, PromptScannerError, ScanInput, ScannerBackendClient, Severity, Span,
    VerdictAction,
};
use toolkit_macros::domain_model;
use toolkit_security::SecurityContext;

use crate::domain::backend::PiiBackend;
use crate::domain::entity::Entity;

#[domain_model]
pub struct Service {
    backend: Arc<dyn PiiBackend>,
    category_prefix: String,
    defer_threshold_words: usize,
    redact_threshold: f32,
    warn_threshold: f32,
}

impl Service {
    #[must_use]
    pub fn new(
        backend: Arc<dyn PiiBackend>,
        category_prefix: String,
        defer_threshold_words: usize,
        redact_threshold: f32,
        warn_threshold: f32,
    ) -> Self {
        Self {
            backend,
            category_prefix,
            defer_threshold_words,
            redact_threshold,
            warn_threshold,
        }
    }
}

/// The verdict action a single detected entity's confidence suggests.
///
/// A PII hit defaults to suggesting `Redact` rather than `Block` — same
/// posture as the retired `pii-plugin` (see `docs/DESIGN.md`).
fn action_for_score(score: f32, redact_threshold: f32, warn_threshold: f32) -> VerdictAction {
    if score >= redact_threshold {
        VerdictAction::Redact
    } else if score >= warn_threshold {
        VerdictAction::Warn
    } else {
        VerdictAction::Allow
    }
}

fn severity_for_action(action: VerdictAction) -> Severity {
    match action {
        VerdictAction::Redact => Severity::High,
        VerdictAction::Warn => Severity::Medium,
        _ => Severity::Low,
    }
}

/// Maps every detected entity to a `Finding` — a message can contain more
/// than one distinct piece of PII (an email *and* a phone number, say), and
/// reporting only the single highest-confidence one would silently miss
/// the rest. Entities that don't even clear `warn_threshold` are dropped
/// rather than reported as noise-level `Allow` findings.
fn findings_from_entities(
    entities: Vec<Entity>,
    category_prefix: &str,
    redact_threshold: f32,
    warn_threshold: f32,
) -> Vec<Finding> {
    entities
        .into_iter()
        .filter_map(|entity| {
            let action = action_for_score(entity.confidence, redact_threshold, warn_threshold);
            if action == VerdictAction::Allow {
                return None;
            }
            Some(Finding {
                detector: String::new(), // prompt-scanner fills this in from the registered vendor
                category: format!("{category_prefix}.{}", entity.category.to_lowercase()),
                severity: severity_for_action(action),
                confidence: entity.confidence,
                span: Some(Span {
                    start: entity.start,
                    end: entity.end,
                }),
                suggested_action: action,
            })
        })
        .collect()
}

#[async_trait]
impl ScannerBackendClient for Service {
    async fn detect(
        &self,
        ctx: &SecurityContext,
        input: ScanInput,
    ) -> Result<BackendOutcome, PromptScannerError> {
        let word_count = input.payload.split_whitespace().count();
        if word_count > self.defer_threshold_words {
            return Ok(BackendOutcome::Defer);
        }

        let entities = self.backend.detect_entities(ctx, &input.payload).await?;
        Ok(BackendOutcome::Findings(findings_from_entities(
            entities,
            &self.category_prefix,
            self.redact_threshold,
            self.warn_threshold,
        )))
    }
}

#[cfg(test)]
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

    fn input(payload: &str) -> ScanInput {
        ScanInput {
            tenant_id: Uuid::nil(),
            content_kind: ContentKind::Prompt,
            payload: payload.to_owned(),
            metadata: HashMap::default(),
        }
    }

    /// Fake backend: returns a fixed set of entities regardless of input,
    /// so `Service`'s orchestration (deferral, threshold-to-action mapping,
    /// `Finding` construction) can be tested without a real model.
    struct FakeBackend {
        entities: Vec<Entity>,
    }

    #[async_trait]
    impl PiiBackend for FakeBackend {
        async fn detect_entities(
            &self,
            _ctx: &SecurityContext,
            _text: &str,
        ) -> Result<Vec<Entity>, PromptScannerError> {
            Ok(self
                .entities
                .iter()
                .map(|e| Entity {
                    category: e.category.clone(),
                    start: e.start,
                    end: e.end,
                    confidence: e.confidence,
                })
                .collect())
        }
    }

    fn service(entities: Vec<Entity>, defer_threshold_words: usize) -> Service {
        Service::new(
            Arc::new(FakeBackend { entities }),
            "pii".to_owned(),
            defer_threshold_words,
            0.6,
            0.3,
        )
    }

    #[test]
    fn action_for_score_picks_redact_at_or_above_threshold() {
        assert_eq!(action_for_score(0.6, 0.6, 0.3), VerdictAction::Redact);
        assert_eq!(action_for_score(0.9, 0.6, 0.3), VerdictAction::Redact);
    }

    #[test]
    fn action_for_score_picks_warn_between_thresholds() {
        assert_eq!(action_for_score(0.3, 0.6, 0.3), VerdictAction::Warn);
    }

    #[test]
    fn action_for_score_picks_allow_below_warn_threshold() {
        assert_eq!(action_for_score(0.0, 0.6, 0.3), VerdictAction::Allow);
    }

    #[test]
    fn severity_matches_action() {
        assert_eq!(severity_for_action(VerdictAction::Redact), Severity::High);
        assert_eq!(severity_for_action(VerdictAction::Warn), Severity::Medium);
        assert_eq!(severity_for_action(VerdictAction::Allow), Severity::Low);
    }

    #[test]
    fn findings_from_entities_of_empty_is_empty() {
        assert!(findings_from_entities(Vec::new(), "pii", 0.6, 0.3).is_empty());
    }

    #[test]
    fn findings_from_entities_drops_below_warn_threshold() {
        let entity = Entity {
            category: "EMAIL".to_owned(),
            start: 0,
            end: 5,
            confidence: 0.1,
        };
        assert!(findings_from_entities(vec![entity], "pii", 0.6, 0.3).is_empty());
    }

    #[tokio::test]
    async fn detect_runs_full_pipeline_and_redacts_high_confidence_pii() {
        let entity = Entity {
            category: "EMAIL".to_owned(),
            start: 8,
            end: 24,
            confidence: 0.95,
        };
        let outcome = service(vec![entity], 100)
            .detect(&ctx(), input("contact jane at example.com now"))
            .await
            .unwrap();

        let BackendOutcome::Findings(findings) = outcome else {
            panic!("expected a Findings outcome");
        };
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].category, "pii.email");
        assert_eq!(findings[0].suggested_action, VerdictAction::Redact);
    }

    #[tokio::test]
    async fn detect_defers_when_payload_exceeds_word_threshold() {
        let outcome = service(Vec::new(), 2)
            .detect(&ctx(), input("contact jane at example.com now"))
            .await
            .unwrap();

        assert!(matches!(outcome, BackendOutcome::Defer));
    }
}
