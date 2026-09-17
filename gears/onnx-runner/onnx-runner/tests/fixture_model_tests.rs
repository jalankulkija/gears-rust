#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Load, verification, inference and dtype handling, against a real ONNX
//! session — the two-thirds of the gear that had no tests at all.
//!
//! Runs against the committed 285-byte fixture, so it needs no network and no
//! 528 MB download. It does need an ONNX Runtime on `ORT_DYLIB_PATH`
//! (`scripts/fetch-onnxruntime.sh`), which CI provisions.

mod common;

use common::{MODEL_ID, fixture_config, fixture_with};
use onnx_runner::infra::ort_session::LoadedModel;
use onnx_runner::infra::tokenizer::LoadedTokenizer;
use onnx_runner_sdk::{ModelFailureReason, NamedTensor, OnnxRunnerError, TensorData};

fn inputs(ids: Vec<i64>, weights: Vec<f32>) -> Vec<NamedTensor> {
    let n = ids.len();
    vec![
        NamedTensor::new("ids", vec![1, n], TensorData::I64(ids)),
        NamedTensor::new("weights", vec![1, n], TensorData::F32(weights)),
    ]
}

// --- Load path -------------------------------------------------------------

#[test]
fn loads_and_verifies_the_real_checksum() {
    // `skip_checksum: false` — the digest in the config is the fixture's real
    // one, so this exercises verification rather than bypassing it.
    let model = LoadedModel::load(&fixture_config()).expect("fixture loads");
    let loaded = model
        .info
        .loaded
        .as_ref()
        .expect("a loaded model reports details");

    assert_eq!(loaded.input_names, vec!["ids", "weights"]);
    assert_eq!(loaded.output_names, vec!["ids_out", "scores"]);
    assert_eq!(loaded.sha256, common::FIXTURE_SHA256);
    assert!(model.info.is_ready());
}

#[test]
fn checksum_mismatch_is_classified() {
    let Err(failure) = LoadedModel::load(&fixture_with(|c| c.sha256 = "0".repeat(64))) else {
        panic!("a wrong digest must not load");
    };
    assert_eq!(failure.reason, ModelFailureReason::ChecksumMismatch);
}

#[test]
fn skip_checksum_is_honoured() {
    let cfg = fixture_with(|c| {
        c.sha256 = "0".repeat(64);
        c.skip_checksum = true;
    });
    assert!(
        LoadedModel::load(&cfg).is_ok(),
        "skip_checksum bypasses verification"
    );
}

#[test]
fn unreadable_file_is_classified() {
    let Err(failure) =
        LoadedModel::load(&fixture_with(|c| c.model_file = "absent.onnx".to_owned()))
    else {
        panic!("a missing file must not load");
    };
    assert_eq!(failure.reason, ModelFailureReason::FileUnreadable);
}

#[test]
fn contract_mismatch_on_inputs_is_classified() {
    let cfg = fixture_with(|c| c.expected_inputs = vec!["not_an_input".to_owned()]);
    let Err(failure) = LoadedModel::load(&cfg) else {
        panic!("a wrong input contract must not load");
    };
    assert_eq!(failure.reason, ModelFailureReason::ContractMismatch);
}

#[test]
fn contract_mismatch_on_outputs_is_classified() {
    let cfg = fixture_with(|c| {
        c.expected_outputs = vec!["not_an_output".to_owned()];
        c.output_dtypes = [(
            "not_an_output".to_owned(),
            onnx_runner::config::OutputDtype::F32,
        )]
        .into_iter()
        .collect();
    });
    let Err(failure) = LoadedModel::load(&cfg) else {
        panic!("a wrong output contract must not load");
    };
    assert_eq!(failure.reason, ModelFailureReason::ContractMismatch);
}

/// The correctness bug this gear had: declaring a dtype the graph does not
/// emit. It must fail at load, not hand a caller reinterpreted numbers.
#[test]
fn declared_dtype_disagreeing_with_the_graph_fails_at_load() {
    let cfg = fixture_with(|c| {
        // `scores` is f32 in the graph.
        c.output_dtypes
            .insert("scores".to_owned(), onnx_runner::config::OutputDtype::I64);
    });
    let Err(failure) = LoadedModel::load(&cfg) else {
        panic!("a dtype lie must not load");
    };
    assert_eq!(failure.reason, ModelFailureReason::ContractMismatch);
    let message = failure.source.to_string();
    assert!(message.contains("scores"), "names the output: {message}");
}

#[test]
fn output_missing_a_declared_dtype_is_an_invalid_config() {
    let cfg = fixture_with(|c| {
        c.output_dtypes.remove("scores");
    });
    let Err(failure) = LoadedModel::load(&cfg) else {
        panic!("an incomplete config must not load");
    };
    assert_eq!(failure.reason, ModelFailureReason::InvalidConfig);
}

#[test]
fn missing_tokenizer_is_classified() {
    let path = common::fixtures_dir().join("absent-tokenizer.json");
    assert!(
        LoadedTokenizer::load(&path).is_err(),
        "a missing tokenizer must not load"
    );
}

// --- Inference path --------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn round_trips_both_dtypes_exactly() {
    let model = LoadedModel::load(&fixture_config()).expect("loads");

    let ids = vec![7_i64, 8, 9];
    let weights = vec![0.5_f32, -1.5, 2.25];
    let out = model
        .run(inputs(ids.clone(), weights.clone()))
        .await
        .expect("inference succeeds");

    assert_eq!(out.len(), 2, "two declared outputs come back");

    // The graph is `Identity`, so equality is exact on every platform — no
    // tolerance to choose, and any drift is a real failure.
    let by_name = |name: &str| {
        out.iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("{name} missing from outputs"))
    };
    assert_eq!(by_name("ids_out").data, TensorData::I64(ids));
    assert_eq!(by_name("scores").data, TensorData::F32(weights));
    assert_eq!(by_name("ids_out").shape, vec![1, 3]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_shape_is_rejected_before_inference() {
    let model = LoadedModel::load(&fixture_config()).expect("loads");

    let bad = vec![NamedTensor::new(
        "ids",
        vec![2, 3],
        TensorData::I64(vec![1; 5]),
    )];
    let err = model
        .run(bad)
        .await
        .expect_err("a bad shape must be rejected");
    assert!(
        matches!(err, OnnxRunnerError::InvalidInput { .. }),
        "got {err:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_declared_input_is_a_contract_error() {
    let model = LoadedModel::load(&fixture_config()).expect("loads");

    // `weights` omitted.
    let partial = vec![NamedTensor::new(
        "ids",
        vec![1, 2],
        TensorData::I64(vec![1, 2]),
    )];
    let err = model
        .run(partial)
        .await
        .expect_err("an incomplete call must fail");
    assert!(
        matches!(err, OnnxRunnerError::ContractMismatch { .. }),
        "got {err:?}"
    );
}

/// A pool of replicas must serve every call, not just the first.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_replica_in_a_pool_serves() {
    let model =
        std::sync::Arc::new(LoadedModel::load(&fixture_with(|c| c.replicas = 3)).expect("loads"));
    assert_eq!(model.live_replicas(), 3);

    // More calls than replicas, so round-robin visits each at least twice.
    let calls: Vec<_> = (0..9_i64)
        .map(|i| {
            let model = std::sync::Arc::clone(&model);
            let weight = f32::from(i16::try_from(i).expect("small test index"));
            tokio::spawn(async move { model.run(inputs(vec![i], vec![weight])).await })
        })
        .collect();

    for call in calls {
        assert!(
            call.await.expect("task joins").is_ok(),
            "every call succeeds"
        );
    }
    assert_eq!(model.live_replicas(), 3, "the pool is intact afterwards");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tokenizer_round_trips_against_the_fixture() {
    let tokenizer = LoadedTokenizer::load(&common::tokenizer_path()).expect("loads");
    let enc = tokenizer
        .encode(MODEL_ID, "hello world".to_owned())
        .await
        .expect("encodes");

    assert!(!enc.ids.is_empty());
    assert_eq!(enc.ids.len(), enc.offsets.len());
}

#[test]
fn the_fixture_stays_small() {
    let total: u64 = std::fs::read_dir(common::fixtures_dir())
        .expect("fixtures dir exists")
        .filter_map(Result::ok)
        .filter_map(|e| e.metadata().ok())
        .filter(std::fs::Metadata::is_file)
        .map(|m| m.len())
        .sum();

    assert!(
        total < 100 * 1024,
        "committed fixtures total {total} bytes; the point of a fixture is that it is small"
    );
}
