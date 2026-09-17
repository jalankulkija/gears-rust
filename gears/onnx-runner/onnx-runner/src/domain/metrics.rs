//! The metrics port, named by the domain and adapted in `infra`.
//!
//! Following this repo's DIP convention (`file-storage`, `mini-chat`): the
//! domain declares what is worth measuring, and exactly one infra module knows
//! how to record it. That keeps `opentelemetry` out of the execution paths and
//! makes the instrument set testable without a metrics backend.
//!
//! # The cardinality rule
//!
//! Every label on every method below comes from **configuration or a closed
//! enum** — never from caller input. Metrics backends fail on unbounded label
//! sets, and this gear's callers supply model ids, tensor names and text, any
//! of which would be unbounded. The method signatures are the enforcement:
//! nothing here accepts a free-form string that a caller could influence,
//! except `model_id`, which comes from the operator's config.

use std::time::Duration;

use onnx_runner_sdk::{ModelFailureReason, OnnxRunnerError};

/// How an inference finished. A closed set, so it is safe as a metric label.
///
/// Deliberately coarser than [`OnnxRunnerError`]: an operator watching a
/// dashboard wants to know *which kind of thing is going wrong*, and the
/// variants collapse to five actionable answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InferenceOutcome {
    Ok,
    /// The caller's request was malformed — a shape mismatch, an unknown
    /// tensor name, oversized input. Their bug, not the gear's.
    InvalidInput,
    /// The deadline expired. Load-dependent, and retryable.
    Timeout,
    /// Shed at the in-flight ceiling. Load-dependent, and retryable.
    Overloaded,
    /// The model is not usable, or the graph itself errored.
    Failed,
}

impl InferenceOutcome {
    /// A stable `snake_case` label value.
    #[must_use]
    pub const fn as_label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::InvalidInput => "invalid_input",
            Self::Timeout => "timeout",
            Self::Overloaded => "overloaded",
            Self::Failed => "failed",
        }
    }
}

impl From<&OnnxRunnerError> for InferenceOutcome {
    fn from(err: &OnnxRunnerError) -> Self {
        match err {
            OnnxRunnerError::InvalidInput { .. } | OnnxRunnerError::ContractMismatch { .. } => {
                Self::InvalidInput
            }
            OnnxRunnerError::Timeout { .. } => Self::Timeout,
            OnnxRunnerError::Overloaded { .. } => Self::Overloaded,
            OnnxRunnerError::UnknownModel { .. }
            | OnnxRunnerError::ModelUnavailable { .. }
            | OnnxRunnerError::InferenceFailed { .. }
            | OnnxRunnerError::TokenizationFailed { .. }
            | OnnxRunnerError::Internal(_) => Self::Failed,
        }
    }
}

/// How a model load finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadOutcome {
    Ok,
    /// Failed, with the classification consumers already branch on.
    Failed(ModelFailureReason),
}

impl LoadOutcome {
    #[must_use]
    pub const fn as_label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failed(reason) => reason.as_code(),
        }
    }
}

/// What this gear reports about itself.
///
/// Queue wait and execution time are separate methods rather than one total,
/// because they have different causes and different remedies: queue wait means
/// too few replicas or too much concurrency, execution time means the model or
/// the thread budget. A single number conceals which.
pub trait OnnxRunnerMetricsPort: Send + Sync {
    /// One inference finished. `queue_wait` is time spent waiting for a
    /// replica permit; `execution` is time inside ONNX Runtime.
    fn record_inference(
        &self,
        model_id: &str,
        outcome: InferenceOutcome,
        queue_wait: Duration,
        execution: Duration,
    );

    /// One tokenization finished.
    fn record_tokenize(&self, model_id: &str, outcome: InferenceOutcome, elapsed: Duration);

    /// In-flight inferences for a model changed by `delta` (+1 on start, -1 on
    /// finish).
    fn record_in_flight_delta(&self, model_id: &str, delta: i64);

    /// A model finished loading, successfully or not. `resident_bytes` is the
    /// estimated footprint (`replicas × file size`) — a floor, not a
    /// measurement: it excludes ONNX Runtime's arenas and per-session thread
    /// stacks. Enough to make an oversized configuration visible without a
    /// profiler.
    fn record_model_load(
        &self,
        model_id: &str,
        outcome: LoadOutcome,
        elapsed: Duration,
        resident_bytes: u64,
    );

    /// A replica lost to a panicking inference was rebuilt, or the rebuild
    /// failed.
    fn record_session_rebuild(&self, model_id: &str, succeeded: bool);
}

/// A metrics port that records nothing.
///
/// The default, so metrics are never a prerequisite for constructing anything
/// and every test does not need a meter.
pub struct NoopMetrics;

impl OnnxRunnerMetricsPort for NoopMetrics {
    fn record_inference(&self, _: &str, _: InferenceOutcome, _: Duration, _: Duration) {}
    fn record_tokenize(&self, _: &str, _: InferenceOutcome, _: Duration) {}
    fn record_in_flight_delta(&self, _: &str, _: i64) {}
    fn record_model_load(&self, _: &str, _: LoadOutcome, _: Duration, _: u64) {}
    fn record_session_rebuild(&self, _: &str, _: bool) {}
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn every_error_variant_maps_to_an_outcome() {
        let cases = [
            (
                OnnxRunnerError::InvalidInput {
                    model_id: "m".to_owned(),
                    tensor: "t".to_owned(),
                    message: String::new(),
                },
                InferenceOutcome::InvalidInput,
            ),
            (
                OnnxRunnerError::Timeout {
                    model_id: "m".to_owned(),
                    elapsed_ms: 1,
                },
                InferenceOutcome::Timeout,
            ),
            (
                OnnxRunnerError::Overloaded {
                    model_id: "m".to_owned(),
                    in_flight: 1,
                },
                InferenceOutcome::Overloaded,
            ),
            (
                OnnxRunnerError::ModelUnavailable {
                    model_id: "m".to_owned(),
                    reason: ModelFailureReason::FileUnreadable,
                },
                InferenceOutcome::Failed,
            ),
            (
                OnnxRunnerError::Internal("x".to_owned()),
                InferenceOutcome::Failed,
            ),
        ];

        for (err, expected) in cases {
            assert_eq!(InferenceOutcome::from(&err), expected, "for {err:?}");
        }
    }

    /// Labels reach a metrics backend, so they must be a closed set. This is
    /// the cardinality rule, asserted.
    #[test]
    fn outcome_labels_are_stable_and_bounded() {
        let all = [
            InferenceOutcome::Ok,
            InferenceOutcome::InvalidInput,
            InferenceOutcome::Timeout,
            InferenceOutcome::Overloaded,
            InferenceOutcome::Failed,
        ];
        let labels: Vec<&str> = all.iter().map(|o| o.as_label()).collect();
        assert_eq!(
            labels,
            ["ok", "invalid_input", "timeout", "overloaded", "failed"]
        );
    }

    #[test]
    fn load_outcome_labels_reuse_the_failure_classification() {
        assert_eq!(LoadOutcome::Ok.as_label(), "ok");
        assert_eq!(
            LoadOutcome::Failed(ModelFailureReason::ChecksumMismatch).as_label(),
            "checksum_mismatch"
        );
    }
}
