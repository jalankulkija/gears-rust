#![cfg(feature = "real-model-tests")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Every call is bounded: validated before work starts, deadlined, and capped
//! in flight.
//!
//! Needs a real model (`scripts/fetch-models.sh`) and a compatible ONNX
//! Runtime (`scripts/fetch-onnxruntime.sh`) — proving a deadline fires against
//! real inference requires real inference.

mod common;

use common::no_metrics;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use onnx_runner::config::{ModelConfig, OutputDtype, OverloadPolicy};
use onnx_runner::gear::{OnnxRunnerLocalClient, load_models};
use onnx_runner_sdk::OnnxRunnerClientV1;
use onnx_runner_sdk::{NamedTensor, OnnxRunnerError, TensorData};
use tokio_util::sync::CancellationToken;
use toolkit_security::SecurityContext;
use uuid::Uuid;

const MODEL: &str = "bounds-model";

fn ctx() -> SecurityContext {
    SecurityContext::builder()
        .subject_id(Uuid::new_v4())
        .subject_tenant_id(Uuid::new_v4())
        .build()
        .expect("valid SecurityContext")
}

fn model(mutate: impl FnOnce(&mut ModelConfig)) -> ModelConfig {
    let mut cfg = ModelConfig {
        model_id: MODEL.to_owned(),
        model_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../models/gliner2-pii"),
        model_file: "model.quantized.onnx".to_owned(),
        skip_checksum: true,
        expected_inputs: vec![
            "input_ids".to_owned(),
            "attention_mask".to_owned(),
            "text_word_indices".to_owned(),
            "schema_special_indices".to_owned(),
        ],
        expected_outputs: vec!["span_probs".to_owned(), "count_logits".to_owned()],
        output_dtypes: [
            ("span_probs".to_owned(), OutputDtype::F32),
            ("count_logits".to_owned(), OutputDtype::F32),
        ]
        .into_iter()
        .collect(),
        replicas: 1,
        intra_threads: 1,
        ..ModelConfig::default()
    };
    mutate(&mut cfg);
    cfg
}

async fn client(cfg: ModelConfig) -> Arc<OnnxRunnerLocalClient> {
    let entries = load_models(vec![cfg], true, no_metrics(), &CancellationToken::new())
        .await
        .expect("the model must load");
    Arc::new(OnnxRunnerLocalClient::new(entries))
}

/// The four declared input tensors, shaped to the ranks this graph actually
/// wants. Getting these right took reading the runtime's rank complaints — a
/// reminder of why synthetic inputs are not something to generate blindly (see
/// the warmup deferral in `model-lifecycle.md` §7).
fn valid_inputs(tokens: usize) -> Vec<NamedTensor> {
    vec![
        NamedTensor::new(
            "input_ids",
            vec![1, tokens],
            TensorData::I64(vec![1; tokens]),
        ),
        NamedTensor::new(
            "attention_mask",
            vec![1, tokens],
            TensorData::I64(vec![1; tokens]),
        ),
        NamedTensor::new(
            "text_word_indices",
            vec![tokens],
            TensorData::I64(vec![0; tokens]),
        ),
        NamedTensor::new(
            "schema_special_indices",
            vec![4],
            TensorData::I64(vec![0; 4]),
        ),
    ]
}

/// Rejected before any allocation proportional to the declared shape.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shape_disagreeing_with_data_is_rejected() {
    let client = client(model(|_| {})).await;

    let bad = vec![NamedTensor::new(
        "input_ids",
        vec![2, 3],
        TensorData::I64(vec![1; 5]),
    )];

    let err = client.run(&ctx(), MODEL, bad).await.unwrap_err();
    assert!(
        matches!(err, OnnxRunnerError::InvalidInput { ref tensor, .. } if tensor == "input_ids"),
        "expected InvalidInput naming the tensor, got {err:?}"
    );
    assert!(
        !err.is_retryable(),
        "a malformed input is not worth retrying"
    );
}

/// A tensor the model never declared must fail as a contract error, citing
/// both sets — not as an opaque error from deep inside `ort`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn undeclared_input_name_is_a_contract_error() {
    let client = client(model(|_| {})).await;

    let mut inputs = valid_inputs(8);
    inputs.push(NamedTensor::new(
        "surprise",
        vec![1],
        TensorData::I64(vec![0]),
    ));

    let err = client.run(&ctx(), MODEL, inputs).await.unwrap_err();
    let message = err.to_string();
    assert!(
        matches!(err, OnnxRunnerError::ContractMismatch { .. }),
        "expected ContractMismatch, got {err:?}"
    );
    assert!(
        message.contains("surprise"),
        "cites the offender: {message}"
    );
}

/// Oversized text is rejected, never silently truncated.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_text_is_rejected_not_truncated() {
    let client = client(model(|c| c.max_text_bytes = 16)).await;

    let err = client
        .encode(&ctx(), MODEL, &"x".repeat(64))
        .await
        .unwrap_err();
    assert!(
        matches!(err, OnnxRunnerError::InvalidInput { .. }),
        "expected InvalidInput, got {err:?}"
    );

    // Under the limit still works, so the guard is a bound and not a wall.
    assert!(client.encode(&ctx(), MODEL, "short").await.is_ok());
}

/// A deadline that cannot be met returns `Timeout`, and — the part worth
/// asserting — the replica is usable again afterwards without a rebuild.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn impossible_deadline_times_out_and_the_replica_recovers() {
    let impatient = client(model(|c| c.inference_timeout = Duration::from_millis(1))).await;

    let err = impatient
        .run(&ctx(), MODEL, valid_inputs(64))
        .await
        .unwrap_err();
    assert!(
        matches!(err, OnnxRunnerError::Timeout { .. }),
        "expected Timeout, got {err:?}"
    );
    assert!(err.is_retryable(), "a deadline miss is load-dependent");

    // The abandoned inference is still running; a generous deadline must
    // eventually succeed on the same model rather than the model being wedged.
    let recovered = client(model(|c| c.inference_timeout = Duration::from_mins(1))).await;
    assert!(
        recovered.run(&ctx(), MODEL, valid_inputs(16)).await.is_ok(),
        "the model must still serve after a timeout"
    );
}

/// Under `Reject`, exceeding the ceiling sheds load rather than queueing
/// without bound.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exceeding_the_in_flight_ceiling_sheds_load() {
    let client = client(model(|c| {
        c.max_in_flight = Some(1);
        c.on_overload = OverloadPolicy::Reject;
        c.inference_timeout = Duration::from_mins(1);
    }))
    .await;

    // Enough concurrent calls that at least one must be turned away.
    let calls: Vec<_> = (0..8)
        .map(|_| {
            let client = Arc::clone(&client);
            tokio::spawn(async move { client.run(&ctx(), MODEL, valid_inputs(64)).await })
        })
        .collect();

    let mut overloaded = 0;
    for call in calls {
        if let Err(OnnxRunnerError::Overloaded { .. }) = call.await.expect("task joins") {
            overloaded += 1;
        }
    }

    assert!(
        overloaded > 0,
        "with a ceiling of 1 and 8 concurrent calls, some must be shed"
    );
}
