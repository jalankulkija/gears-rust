#![cfg(feature = "real-model-tests")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! One bad model artifact must not stop the gear, and every configured model
//! must be observable with a state and a stable failure code.
//!
//! Needs a real model on disk (`scripts/fetch-models.sh`) and a compatible
//! ONNX Runtime (`scripts/fetch-onnxruntime.sh`), because proving a *good*
//! model keeps serving alongside a broken one requires a good model.

mod common;

use common::no_metrics;
use std::path::PathBuf;

use onnx_runner::config::{ModelConfig, OutputDtype};
use onnx_runner::gear::{OnnxRunnerLocalClient, load_models};
use onnx_runner_sdk::{
    ModelFailureReason, ModelInfo, ModelState, OnnxRunnerClientV1, OnnxRunnerError,
};
use tokio_util::sync::CancellationToken;
use toolkit_security::SecurityContext;
use uuid::Uuid;

fn ctx() -> SecurityContext {
    SecurityContext::builder()
        .subject_id(Uuid::new_v4())
        .subject_tenant_id(Uuid::new_v4())
        .build()
        .expect("valid SecurityContext")
}

fn models_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../models/gliner2-pii")
}

/// A model that loads. Single replica: these tests care about lifecycle, not
/// concurrency, and a 446 MB model is worth loading once.
fn good_model(model_id: &str) -> ModelConfig {
    ModelConfig {
        model_id: model_id.to_owned(),
        model_dir: models_dir(),
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
    }
}

fn with(model_id: &str, mutate: impl FnOnce(&mut ModelConfig)) -> ModelConfig {
    let mut cfg = good_model(model_id);
    mutate(&mut cfg);
    cfg
}

/// Loads, then reads back through the real `list_models` projection rather
/// than asserting on internal state.
async fn load(models: Vec<ModelConfig>, require_all: bool) -> Result<Vec<ModelInfo>, String> {
    let entries = load_models(models, require_all, no_metrics(), &CancellationToken::new())
        .await
        .map_err(|e| e.to_string())?;
    OnnxRunnerLocalClient::new(entries)
        .list_models(&ctx())
        .await
        .map_err(|e| e.to_string())
}

fn state_of<'a>(infos: &'a [ModelInfo], model_id: &str) -> &'a ModelState {
    &infos
        .iter()
        .find(|i| i.model_id == model_id)
        .unwrap_or_else(|| panic!("{model_id} must appear in list_models"))
        .state
}

/// The headline behaviour: a broken artifact costs that model, not the process.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_bad_model_does_not_stop_the_good_one() {
    let infos = load(
        vec![
            good_model("good"),
            with("broken", |c| {
                c.model_file = "does-not-exist.onnx".to_owned()
            }),
        ],
        false,
    )
    .await
    .expect("the gear must start despite a broken model");

    assert_eq!(infos.len(), 2, "every configured model is reported");
    assert!(
        state_of(&infos, "good").is_ready(),
        "the good model still serves"
    );
    assert_eq!(
        state_of(&infos, "broken"),
        &ModelState::Unavailable {
            reason: ModelFailureReason::FileUnreadable
        }
    );
}

/// Each failure mode must produce its own stable code — a consumer branches on
/// these, and an operator greps for them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_failure_mode_has_its_own_reason() {
    let cases = [
        (
            "missing-file",
            with("missing-file", |c| c.model_file = "nope.onnx".to_owned()),
            ModelFailureReason::FileUnreadable,
        ),
        (
            "bad-checksum",
            with("bad-checksum", |c| {
                c.skip_checksum = false;
                c.sha256 = "0".repeat(64);
            }),
            ModelFailureReason::ChecksumMismatch,
        ),
        (
            "renamed-tensor",
            with("renamed-tensor", |c| {
                // Self-consistent config — a dtype is declared for the renamed
                // output — so this reaches the graph check rather than being
                // rejected earlier as an incomplete config.
                c.expected_outputs = vec!["not_a_real_output".to_owned()];
                c.output_dtypes = [("not_a_real_output".to_owned(), OutputDtype::F32)]
                    .into_iter()
                    .collect();
            }),
            ModelFailureReason::ContractMismatch,
        ),
        (
            "bad-tokenizer",
            with("bad-tokenizer", |c| {
                c.tokenizer_file = "model.quantized.onnx".to_owned();
            }),
            ModelFailureReason::TokenizerInvalid,
        ),
        (
            "zero-replicas",
            with("zero-replicas", |c| c.replicas = 0),
            ModelFailureReason::InvalidConfig,
        ),
        (
            "undeclared-dtype",
            with("undeclared-dtype", |c| {
                c.output_dtypes.remove("span_probs");
            }),
            ModelFailureReason::InvalidConfig,
        ),
    ];

    for (model_id, cfg, expected) in cases {
        let infos = load(vec![cfg], false)
            .await
            .expect("startup stays non-fatal");
        assert_eq!(
            state_of(&infos, model_id),
            &ModelState::Unavailable { reason: expected },
            "{model_id} must classify as {expected}"
        );
    }
}

/// The escape hatch for deployments that would rather not start than start
/// degraded.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn require_all_models_restores_fail_fast() {
    let err = load(
        vec![with("broken", |c| c.model_file = "nope.onnx".to_owned())],
        true,
    )
    .await
    .expect_err("require_all_models must fail startup");

    assert!(err.contains("broken"), "the error names the model: {err}");
}

/// A typo and an outage are different problems and must not look the same.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_and_unavailable_are_distinguishable() {
    let entries = load_models(
        vec![with("broken", |c| c.model_file = "nope.onnx".to_owned())],
        false,
        no_metrics(),
        &CancellationToken::new(),
    )
    .await
    .expect("startup stays non-fatal");
    let client = OnnxRunnerLocalClient::new(entries);
    let ctx = ctx();

    let unavailable = client.run(&ctx, "broken", Vec::new()).await.unwrap_err();
    assert!(
        matches!(unavailable, OnnxRunnerError::ModelUnavailable { ref reason, .. }
            if *reason == ModelFailureReason::FileUnreadable),
        "a configured-but-broken model is an outage: {unavailable:?}"
    );

    let unknown = client.run(&ctx, "typo", Vec::new()).await.unwrap_err();
    assert!(
        matches!(unknown, OnnxRunnerError::UnknownModel { .. }),
        "an unconfigured id is a caller mistake: {unknown:?}"
    );
}

/// A gear serving nothing still starts, but must say so loudly.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn zero_loaded_models_still_starts() {
    let infos = load(
        vec![with("broken", |c| c.model_file = "nope.onnx".to_owned())],
        false,
    )
    .await
    .expect("zero ready models is degraded, not fatal");

    assert_eq!(infos.len(), 1);
    assert!(!infos[0].is_ready());
}
