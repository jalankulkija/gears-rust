//! OpenTelemetry adapter for [`OnnxRunnerMetricsPort`].
//!
//! The only file in this gear that imports `opentelemetry`, mirroring
//! `file-storage`'s `infra/metrics.rs`: instruments are obtained from a `Meter`
//! passed in by the gear, not from a global, so the execution paths never reach
//! for ambient state.
//!
//! Counter names omit a `_total` suffix — the Prometheus exporter appends it —
//! matching the convention in `file-storage` and `mini-chat`.

use std::time::Duration;

use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram, Meter, UpDownCounter};

use crate::domain::metrics::{InferenceOutcome, LoadOutcome, OnnxRunnerMetricsPort};

/// OpenTelemetry-backed implementation of [`OnnxRunnerMetricsPort`].
pub struct OnnxRunnerMetricsMeter {
    inference_duration_ms: Histogram<f64>,
    queue_wait_ms: Histogram<f64>,
    tokenize_duration_ms: Histogram<f64>,
    model_load_duration_ms: Histogram<f64>,
    model_resident_bytes: Histogram<f64>,
    calls: Counter<u64>,
    model_load: Counter<u64>,
    session_rebuild: Counter<u64>,
    in_flight: UpDownCounter<i64>,
}

impl OnnxRunnerMetricsMeter {
    /// Builds every instrument. `prefix` is prepended to each metric name.
    #[must_use]
    pub fn new(meter: &Meter, prefix: &str) -> Self {
        Self {
            inference_duration_ms: meter
                .f64_histogram(format!("{prefix}_inference_duration_ms"))
                .with_description(
                    "Time inside ONNX Runtime for one inference, excluding queue wait \
                     (model_id, outcome)",
                )
                .with_unit("ms")
                .build(),
            queue_wait_ms: meter
                .f64_histogram(format!("{prefix}_queue_wait_ms"))
                .with_description(
                    "Time spent waiting for a replica before inference began (model_id)",
                )
                .with_unit("ms")
                .build(),
            tokenize_duration_ms: meter
                .f64_histogram(format!("{prefix}_tokenize_duration_ms"))
                .with_description("Time to tokenize one input (model_id, outcome)")
                .with_unit("ms")
                .build(),
            model_load_duration_ms: meter
                .f64_histogram(format!("{prefix}_model_load_duration_ms"))
                .with_description(
                    "Time to read, verify and build sessions for one model (model_id, outcome)",
                )
                .with_unit("ms")
                .build(),
            model_resident_bytes: meter
                .f64_histogram(format!("{prefix}_model_resident_bytes"))
                .with_description(
                    "Estimated resident footprint at load, replicas x file size (model_id). \
                     A floor: excludes ONNX Runtime arenas and per-session thread stacks",
                )
                .with_unit("By")
                .build(),
            calls: meter
                .u64_counter(format!("{prefix}_calls"))
                .with_description(
                    "Inference and tokenization calls by outcome (model_id, op, outcome)",
                )
                .build(),
            model_load: meter
                .u64_counter(format!("{prefix}_model_load"))
                .with_description("Model load attempts by outcome (model_id, outcome)")
                .build(),
            session_rebuild: meter
                .u64_counter(format!("{prefix}_session_rebuild"))
                .with_description(
                    "Replica rebuilds after a panicking inference, by result (model_id, result)",
                )
                .build(),
            in_flight: meter
                .i64_up_down_counter(format!("{prefix}_in_flight"))
                .with_description("Inferences currently executing or queued (model_id)")
                .build(),
        }
    }
}

/// Milliseconds as `f64`. Lossless at any duration this gear will see, and
/// avoids the checked-conversion noise an integer histogram would need.
fn millis(d: Duration) -> f64 {
    d.as_secs_f64() * 1_000.0
}

impl OnnxRunnerMetricsPort for OnnxRunnerMetricsMeter {
    fn record_inference(
        &self,
        model_id: &str,
        outcome: InferenceOutcome,
        queue_wait: Duration,
        execution: Duration,
    ) {
        let model = KeyValue::new("model_id", model_id.to_owned());
        let result = KeyValue::new("outcome", outcome.as_label());

        self.queue_wait_ms
            .record(millis(queue_wait), std::slice::from_ref(&model));
        self.inference_duration_ms
            .record(millis(execution), &[model.clone(), result.clone()]);
        self.calls
            .add(1, &[model, KeyValue::new("op", "run"), result]);
    }

    fn record_tokenize(&self, model_id: &str, outcome: InferenceOutcome, elapsed: Duration) {
        let model = KeyValue::new("model_id", model_id.to_owned());
        let result = KeyValue::new("outcome", outcome.as_label());

        self.tokenize_duration_ms
            .record(millis(elapsed), &[model.clone(), result.clone()]);
        self.calls
            .add(1, &[model, KeyValue::new("op", "encode"), result]);
    }

    fn record_in_flight_delta(&self, model_id: &str, delta: i64) {
        self.in_flight
            .add(delta, &[KeyValue::new("model_id", model_id.to_owned())]);
    }

    fn record_model_load(
        &self,
        model_id: &str,
        outcome: LoadOutcome,
        elapsed: Duration,
        resident_bytes: u64,
    ) {
        let model = KeyValue::new("model_id", model_id.to_owned());
        let result = KeyValue::new("outcome", outcome.as_label());

        self.model_load_duration_ms
            .record(millis(elapsed), &[model.clone(), result.clone()]);
        if matches!(outcome, LoadOutcome::Ok) {
            // Only meaningful for a model that actually loaded.
            #[allow(clippy::cast_precision_loss)]
            self.model_resident_bytes
                .record(resident_bytes as f64, std::slice::from_ref(&model));
        }
        self.model_load.add(1, &[model, result]);
    }

    fn record_session_rebuild(&self, model_id: &str, succeeded: bool) {
        self.session_rebuild.add(
            1,
            &[
                KeyValue::new("model_id", model_id.to_owned()),
                KeyValue::new("result", if succeeded { "ok" } else { "failed" }),
            ],
        );
    }
}
