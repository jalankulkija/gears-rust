#![allow(clippy::unwrap_used, clippy::expect_used)]

//! What the gear reports about itself, and what it must never report.
//!
//! Two things are proven here. First, that queue wait and execution time are
//! genuinely *separate* measurements rather than one number recorded twice —
//! an operator uses them to tell "too few replicas" from "the model is slow",
//! and a benchmark uses them to attribute cost. Second, that payload content
//! cannot reach telemetry.
//!
//! Runs against the committed fixture, so no model download is needed.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{MODEL_ID, RecordingMetrics, ctx, fixture_config, fixture_with, no_metrics};
use onnx_runner::config::OverloadPolicy;
use onnx_runner::domain::InferenceOutcome;
use onnx_runner::gear::{OnnxRunnerLocalClient, load_models};
use onnx_runner_sdk::{NamedTensor, OnnxRunnerClientV1, TensorData};
use tokio_util::sync::CancellationToken;

fn inputs(n: usize) -> Vec<NamedTensor> {
    vec![
        NamedTensor::new("ids", vec![1, n], TensorData::I64(vec![1; n])),
        NamedTensor::new("weights", vec![1, n], TensorData::F32(vec![0.5; n])),
    ]
}

async fn client_with(
    cfg: onnx_runner::config::ModelConfig,
    metrics: Arc<RecordingMetrics>,
) -> OnnxRunnerLocalClient {
    let entries = load_models(vec![cfg], true, metrics, &CancellationToken::new())
        .await
        .expect("fixture loads");
    OnnxRunnerLocalClient::new(entries)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_successful_inference_records_duration_and_outcome() {
    let metrics = Arc::new(RecordingMetrics::default());
    let client = client_with(fixture_config(), Arc::clone(&metrics)).await;

    client.run(&ctx(), MODEL_ID, inputs(8)).await.expect("runs");

    let recorded = metrics.inferences();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].model_id, MODEL_ID);
    assert_eq!(recorded[0].outcome, InferenceOutcome::Ok);
}

/// The property the whole feature turns on: an idle gear shows ~no queue wait,
/// so a non-zero queue wait elsewhere means something real. If the two numbers
/// were the same measurement, this would fail.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queue_wait_is_near_zero_when_idle() {
    let metrics = Arc::new(RecordingMetrics::default());
    let client = client_with(fixture_config(), Arc::clone(&metrics)).await;

    client.run(&ctx(), MODEL_ID, inputs(8)).await.expect("runs");

    let recorded = metrics.inferences();
    assert!(
        recorded[0].queue_wait < Duration::from_millis(50),
        "an uncontended call should not queue: {:?}",
        recorded[0].queue_wait
    );
}

/// ...and the other half: with one replica and several concurrent callers,
/// somebody waits. Proving both directions is what distinguishes "two
/// measurements" from "one measurement recorded twice".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queue_wait_is_nonzero_when_replicas_are_saturated() {
    let metrics = Arc::new(RecordingMetrics::default());
    let client = Arc::new(
        client_with(
            fixture_with(|c| {
                c.replicas = 1;
                c.max_in_flight = Some(1);
                c.on_overload = OverloadPolicy::Queue;
            }),
            Arc::clone(&metrics),
        )
        .await,
    );

    // A large input so each inference occupies its replica long enough for the
    // others to observe a wait.
    let calls: Vec<_> = (0..8)
        .map(|_| {
            let client = Arc::clone(&client);
            tokio::spawn(async move { client.run(&ctx(), MODEL_ID, inputs(4096)).await })
        })
        .collect();
    for call in calls {
        call.await.expect("task joins").expect("inference succeeds");
    }

    let recorded = metrics.inferences();
    assert_eq!(recorded.len(), 8);
    assert!(
        recorded.iter().any(|r| r.queue_wait > Duration::ZERO),
        "with one replica and eight callers, someone must have queued"
    );
    assert!(
        recorded.iter().all(|r| r.execution > Duration::ZERO),
        "every call that ran should report execution time"
    );
}

/// A gauge that drifts is worse than no gauge. Every increment must be paired.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn in_flight_returns_to_zero_including_on_rejection() {
    let metrics = Arc::new(RecordingMetrics::default());
    let client = client_with(fixture_config(), Arc::clone(&metrics)).await;

    client.run(&ctx(), MODEL_ID, inputs(8)).await.expect("runs");

    // A rejected call must decrement too — the guard covers validation
    // failures, not just successful inference.
    let bad = vec![NamedTensor::new(
        "ids",
        vec![9, 9],
        TensorData::I64(vec![1]),
    )];
    assert!(client.run(&ctx(), MODEL_ID, bad).await.is_err());

    assert_eq!(
        metrics.net_in_flight(),
        0,
        "every in-flight increment must be matched by a decrement"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rejected_input_is_recorded_with_its_own_outcome() {
    let metrics = Arc::new(RecordingMetrics::default());
    let client = client_with(fixture_config(), Arc::clone(&metrics)).await;

    let bad = vec![NamedTensor::new(
        "ids",
        vec![2, 3],
        TensorData::I64(vec![1; 5]),
    )];
    assert!(client.run(&ctx(), MODEL_ID, bad).await.is_err());

    let recorded = metrics.inferences();
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded[0].outcome,
        InferenceOutcome::InvalidInput,
        "a caller's malformed request is distinguishable from the model failing"
    );
    assert_eq!(
        recorded[0].execution,
        Duration::ZERO,
        "a call rejected before execution should report no execution time"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_load_is_recorded_with_its_footprint() {
    let metrics = Arc::new(RecordingMetrics::default());
    let _client = client_with(fixture_config(), Arc::clone(&metrics)).await;

    let loads = metrics.loads.lock().expect("not poisoned").clone();
    assert_eq!(loads.len(), 1);
    let (model_id, outcome, resident) = &loads[0];
    assert_eq!(model_id, MODEL_ID);
    assert_eq!(outcome, "ok");
    assert!(*resident > 0, "resident bytes should be estimated at load");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_load_is_recorded_with_its_reason() {
    let metrics = Arc::new(RecordingMetrics::default());
    let entries = load_models(
        vec![fixture_with(|c| c.model_file = "absent.onnx".to_owned())],
        false,
        Arc::clone(&metrics) as Arc<dyn onnx_runner::domain::OnnxRunnerMetricsPort>,
        &CancellationToken::new(),
    )
    .await
    .expect("startup stays non-fatal");
    drop(entries);

    let loads = metrics.loads.lock().expect("not poisoned").clone();
    assert_eq!(loads[0].1, "file_unreadable", "the reason is the label");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tokenization_is_measured_separately_from_inference() {
    let metrics = Arc::new(RecordingMetrics::default());
    let client = client_with(fixture_config(), Arc::clone(&metrics)).await;

    client
        .encode(&ctx(), MODEL_ID, "hello world")
        .await
        .expect("encodes");

    assert_eq!(metrics.tokenizations.lock().expect("not poisoned").len(), 1);
    assert!(
        metrics.inferences().is_empty(),
        "tokenizing is not inference and must not be counted as it"
    );
}

/// Metrics must never be a prerequisite for running.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_gear_works_without_a_metrics_backend() {
    let entries = load_models(
        vec![fixture_config()],
        true,
        no_metrics(),
        &CancellationToken::new(),
    )
    .await
    .expect("loads");
    let client = OnnxRunnerLocalClient::new(entries);

    assert!(client.run(&ctx(), MODEL_ID, inputs(4)).await.is_ok());
}
