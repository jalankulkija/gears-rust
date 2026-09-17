//! Operator-visible readiness, projected from per-model state.
//!
//! This is the *only* thing this gear puts in front of an operator. It owns no
//! tenant-facing resource and serves no HTTP routes of its own
//! (`cpt-cf-onnx-runner-constraint-no-http-surface`), so readiness goes through
//! the platform [`Healthcheck`] registry, which API Gateway aggregates into
//! `/health` and `/readyz`.
//!
//! # What it must never say
//!
//! `/health` is **unauthenticated in every serving mode**. The registry
//! sanitizes messages, but this check must not rely on that: it reports
//! operator-chosen model ids and the stable [`ModelFailureReason`] codes, and
//! nothing else. No filesystem paths, no tensor data, no tokenized text
//! (`cpt-cf-onnx-runner-principle-no-payload-in-telemetry`).

use std::sync::Arc;

use async_trait::async_trait;
use onnx_runner_sdk::{ModelInfo, ModelState};
use toolkit::{Healthcheck, HealthcheckResult};

use crate::gear::OnnxRunnerLocalClient;

/// Readiness for every configured model.
///
/// One composite check for the gear, per the platform's guidance, rather than
/// one probe per model: the aggregate is what decides whether the pod stays in
/// traffic.
pub struct ModelReadiness {
    client: Arc<OnnxRunnerLocalClient>,
}

impl ModelReadiness {
    /// `#[doc(hidden)] pub` for the same reason as [`crate::gear::load_models`]:
    /// `tests/healthcheck_tests.rs` drives the real check over a real model
    /// map, and an integration test cannot reach a crate-private item. Not
    /// public API — the platform obtains this through
    /// [`toolkit::contracts::RestApiCapability::healthcheck`].
    #[doc(hidden)]
    #[must_use]
    pub const fn new(client: Arc<OnnxRunnerLocalClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Healthcheck for ModelReadiness {
    fn name(&self) -> &'static str {
        "onnx-runner-models"
    }

    async fn check(&self) -> HealthcheckResult {
        // Reads recorded state only. No session is touched and no inference
        // runs: a readiness probe that exercised the thing it reports on would
        // occupy a replica every few seconds and could itself cause the
        // overload it is meant to detect.
        evaluate(&self.client.model_snapshot())
    }
}

/// Turns a model snapshot into a verdict.
///
/// Free function over the snapshot rather than a method, so the whole decision
/// table is testable without loading a model.
///
/// | Snapshot | Verdict |
/// |---|---|
/// | No models configured | `Unhealthy` — a runner with nothing to run |
/// | No model ready | `Unhealthy` — configured, none usable |
/// | Some ready, some not | `Degraded` — kept in rotation, serves what it has |
/// | All ready | `Healthy` |
///
/// The first row is the defect this check exists to fix: a gear serving zero
/// models used to be indistinguishable from a healthy one, because nothing
/// asked.
fn evaluate(models: &[ModelInfo]) -> HealthcheckResult {
    if models.is_empty() {
        return HealthcheckResult::unhealthy("no models configured")
            .with_code("no_models_configured");
    }

    let total = models.len();
    let ready = models.iter().filter(|m| m.is_ready()).count();

    if ready == total {
        return HealthcheckResult::healthy();
    }

    let detail = describe_not_ready(models);

    if ready == 0 {
        // Nothing this gear can serve. Every call will fail, so it must leave
        // rotation rather than absorb traffic it cannot answer.
        HealthcheckResult::unhealthy(format!("no models ready ({total} configured): {detail}"))
            .with_code("no_models_ready")
    } else {
        // Degraded keeps the pod in traffic, which is right: the models that
        // did load still serve, and consumers of an unavailable one get a
        // typed `ModelUnavailable` they are required to map to failed
        // coverage.
        HealthcheckResult::degraded(format!(
            "{} of {total} models not ready: {detail}",
            total - ready
        ))
        .with_code("models_unavailable")
    }
}

/// Names each not-ready model and why, as `id (reason_code)`.
///
/// Model ids come from operator configuration and reason codes are the stable
/// `snake_case` classification — both safe on an unauthenticated endpoint. The
/// underlying diagnostic, which may name a path, is deliberately not here; it
/// went to the log at load time.
fn describe_not_ready(models: &[ModelInfo]) -> String {
    models
        .iter()
        .filter(|m| !m.is_ready())
        .map(|m| match m.state {
            ModelState::Unavailable { reason } => format!("{} ({reason})", m.model_id),
            // Transient: loading at startup, or rebuilding a session lost to a
            // panicking inference.
            ModelState::Loading => format!("{} (loading)", m.model_id),
            ModelState::Ready => unreachable!("filtered above"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use onnx_runner_sdk::{LoadedModelInfo, ModelFailureReason};
    use toolkit::HealthcheckStatus;

    fn ready(model_id: &str) -> ModelInfo {
        ModelInfo {
            model_id: model_id.to_owned(),
            state: ModelState::Ready,
            loaded: Some(LoadedModelInfo {
                input_names: vec!["input_ids".to_owned()],
                output_names: vec!["logits".to_owned()],
                sha256: "0".repeat(64),
                replicas: 2,
                labels: None,
            }),
        }
    }

    fn unavailable(model_id: &str, reason: ModelFailureReason) -> ModelInfo {
        ModelInfo {
            model_id: model_id.to_owned(),
            state: ModelState::Unavailable { reason },
            loaded: None,
        }
    }

    #[test]
    fn all_ready_is_healthy() {
        let r = evaluate(&[ready("a"), ready("b")]);
        assert_eq!(r.status, HealthcheckStatus::Healthy);
        assert!(r.message.is_none(), "a healthy check needs no explanation");
    }

    #[test]
    fn zero_models_configured_is_unhealthy_not_healthy_by_vacuity() {
        // The defect this whole check exists for: nothing loaded used to look
        // exactly like everything loaded.
        let r = evaluate(&[]);
        assert_eq!(r.status, HealthcheckStatus::Unhealthy);
        assert_eq!(r.code.as_deref(), Some("no_models_configured"));
    }

    #[test]
    fn one_unavailable_of_several_is_degraded_and_names_it() {
        let r = evaluate(&[
            ready("gliner2-pii"),
            unavailable("piiranha-v1", ModelFailureReason::ChecksumMismatch),
        ]);

        assert_eq!(
            r.status,
            HealthcheckStatus::Degraded,
            "still serves gliner2-pii"
        );
        assert_eq!(r.code.as_deref(), Some("models_unavailable"));

        let msg = r.message.expect("degraded explains itself");
        assert!(msg.contains("piiranha-v1"), "names the model: {msg}");
        assert!(msg.contains("checksum_mismatch"), "names the reason: {msg}");
        assert!(
            !msg.contains("gliner2-pii"),
            "does not list working models: {msg}"
        );
    }

    #[test]
    fn every_model_unavailable_is_unhealthy() {
        let r = evaluate(&[
            unavailable("a", ModelFailureReason::FileUnreadable),
            unavailable("b", ModelFailureReason::SessionBuildFailed),
        ]);

        assert_eq!(
            r.status,
            HealthcheckStatus::Unhealthy,
            "nothing left to serve"
        );
        assert_eq!(r.code.as_deref(), Some("no_models_ready"));

        let msg = r.message.expect("unhealthy explains itself");
        assert!(msg.contains("a (file_unreadable)"), "{msg}");
        assert!(msg.contains("b (session_build_failed)"), "{msg}");
    }

    #[test]
    fn loading_counts_as_not_ready() {
        // A model rebuilding a session lost to a panic is not servable yet,
        // but it is not a failure either — so it degrades rather than fails.
        let loading = ModelInfo {
            model_id: "rebuilding".to_owned(),
            state: ModelState::Loading,
            loaded: None,
        };
        let r = evaluate(&[ready("a"), loading]);

        assert_eq!(r.status, HealthcheckStatus::Degraded);
        let msg = r.message.expect("degraded explains itself");
        assert!(msg.contains("rebuilding (loading)"), "{msg}");
    }

    #[test]
    fn the_message_carries_no_paths_or_payload() {
        // The sentinel for an unauthenticated endpoint: only operator-chosen
        // ids and stable reason codes may appear.
        let r = evaluate(&[unavailable(
            "gliner2-pii",
            ModelFailureReason::TokenizerInvalid,
        )]);
        let msg = r.message.expect("unhealthy explains itself");

        assert!(!msg.contains('/'), "no filesystem path may appear: {msg}");
        assert!(
            !msg.contains(".onnx"),
            "no artifact filename may appear: {msg}"
        );
        assert_eq!(
            msg, "no models ready (1 configured): gliner2-pii (tokenizer_invalid)",
            "the whole message, pinned"
        );
    }

    #[test]
    fn reason_codes_match_the_sdk_spelling() {
        // Keeps this projection tied to `ModelFailureReason::as_code`, which is
        // also the wire and telemetry spelling, so the three cannot drift.
        for reason in [
            ModelFailureReason::FileUnreadable,
            ModelFailureReason::ChecksumMismatch,
            ModelFailureReason::ContractMismatch,
            ModelFailureReason::TokenizerInvalid,
            ModelFailureReason::SessionBuildFailed,
            ModelFailureReason::InvalidConfig,
        ] {
            let msg = evaluate(&[unavailable("m", reason)])
                .message
                .expect("unhealthy explains itself");
            assert!(msg.contains(reason.as_code()), "{reason:?} -> {msg}");
        }
    }
}
