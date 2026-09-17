#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Hosting several distinct models at once.
//!
//! The gear's whole purpose is being a *generic* runner, and that claim has
//! never been executed: until now every test loaded one model. This suite
//! hosts two graphs that deliberately disagree — different tensor names,
//! different arity of dtypes, different digests — because two copies of the
//! same model would pass a contract-isolation test that no isolation would
//! also pass.
//!
//! Its external acceptance test lives outside this repo: a consumer must be
//! able to add a second model by **configuration only**, with no source change
//! here. Everything below is written against the public config and client
//! surfaces for that reason.

mod common;

use std::sync::Arc;
use std::time::Instant;

use common::{
    MODEL_ID, OTHER_MODEL_ID, ctx, fixture_config, no_metrics, other_fixture_config,
    other_fixture_with, resident_kb,
};
use onnx_runner::gear::{OnnxRunnerLocalClient, load_models};
use onnx_runner::{NamedTensor, OnnxRunnerClientV1, OnnxRunnerError, TensorData};
use tokio_util::sync::CancellationToken;

/// Both fixtures, loaded exactly as the gear loads them.
async fn two_model_client() -> OnnxRunnerLocalClient {
    client_over(vec![fixture_config(), other_fixture_config()]).await
}

async fn client_over(configs: Vec<onnx_runner::config::ModelConfig>) -> OnnxRunnerLocalClient {
    let models = load_models(configs, false, no_metrics(), &CancellationToken::new())
        .await
        .expect("permissive load never fails");
    OnnxRunnerLocalClient::new(models)
}

fn tiny_inputs(n: usize) -> Vec<NamedTensor> {
    let ids: Vec<i64> = sequential_ids(n);
    let weights: Vec<f32> = (0..n).map(|i| f32::from(u8_of(i)) * 0.5).collect();
    vec![
        NamedTensor::new("ids", vec![1, n], TensorData::I64(ids)),
        NamedTensor::new("weights", vec![1, n], TensorData::F32(weights)),
    ]
}

/// `0..n` as `i64`, without an `as` cast — the request path forbids them, and
/// a test that builds its inputs should not model what the gear refuses to do.
fn sequential_ids(n: usize) -> Vec<i64> {
    (0..n)
        .map(|i| i64::try_from(i).expect("test sizes are small"))
        .collect()
}

fn u8_of(i: usize) -> u8 {
    u8::try_from(i % 251).unwrap_or(0)
}

fn other_inputs(n: usize) -> Vec<NamedTensor> {
    let tokens: Vec<u8> = (0..n).map(u8_of).collect();
    let marks: Vec<bool> = (0..n).map(|i| i % 3 == 0).collect();
    vec![
        NamedTensor::new("tokens", vec![1, n], TensorData::U8(tokens)),
        NamedTensor::new("marks", vec![1, n], TensorData::Bool(marks)),
    ]
}

fn named<'a>(out: &'a [NamedTensor], name: &str) -> &'a NamedTensor {
    out.iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("{name} missing from outputs"))
}

// --- Both models hosted, each with its own everything -----------------------

#[tokio::test]
async fn both_models_load_with_their_own_contracts_and_digests() {
    let client = two_model_client().await;
    let models = client.list_models(&ctx()).await.expect("lists");

    assert_eq!(models.len(), 2, "both configured models are reported");

    let tiny = models
        .iter()
        .find(|m| m.model_id == MODEL_ID)
        .expect("tiny listed");
    let other = models
        .iter()
        .find(|m| m.model_id == OTHER_MODEL_ID)
        .expect("other listed");

    assert!(tiny.is_ready() && other.is_ready(), "both loaded");

    let tiny_loaded = tiny.loaded.as_ref().expect("ready implies loaded detail");
    let other_loaded = other.loaded.as_ref().expect("ready implies loaded detail");

    assert_eq!(tiny_loaded.input_names, vec!["ids", "weights"]);
    assert_eq!(tiny_loaded.output_names, vec!["ids_out", "scores"]);
    assert_eq!(other_loaded.input_names, vec!["tokens", "marks"]);
    assert_eq!(other_loaded.output_names, vec!["tokens_out", "marks_out"]);

    assert_eq!(tiny_loaded.sha256, common::FIXTURE_SHA256);
    assert_eq!(other_loaded.sha256, common::OTHER_FIXTURE_SHA256);
    assert_ne!(
        tiny_loaded.sha256, other_loaded.sha256,
        "distinct artifacts, verified independently"
    );
}

#[tokio::test]
async fn each_model_runs_against_its_own_graph() {
    let client = two_model_client().await;

    let tiny_out = client
        .run(&ctx(), MODEL_ID, tiny_inputs(4))
        .await
        .expect("tiny runs");
    assert_eq!(
        named(&tiny_out, "ids_out").data,
        TensorData::I64(vec![0, 1, 2, 3])
    );
    assert_eq!(
        named(&tiny_out, "scores").data,
        TensorData::F32(vec![0.0, 0.5, 1.0, 1.5])
    );

    let other_out = client
        .run(&ctx(), OTHER_MODEL_ID, other_inputs(4))
        .await
        .expect("other runs");
    assert_eq!(
        named(&other_out, "tokens_out").data,
        TensorData::U8(vec![0, 1, 2, 3])
    );
    assert_eq!(
        named(&other_out, "marks_out").data,
        TensorData::Bool(vec![true, false, false, true])
    );
}

#[tokio::test]
async fn one_models_tensors_are_rejected_by_the_other() {
    // The isolation that matters: dispatching by id must not mean "any
    // configured graph will do". Feeding tiny's inputs to `other` is a
    // contract violation, not a shape error discovered inside ONNX Runtime.
    let client = two_model_client().await;
    let err = client
        .run(&ctx(), OTHER_MODEL_ID, tiny_inputs(2))
        .await
        .expect_err("wrong model's tensors must be rejected");

    let rendered = err.to_string();
    assert!(
        rendered.contains(OTHER_MODEL_ID),
        "names the model that rejected them: {rendered}"
    );
    assert!(
        !err.is_retryable(),
        "a contract violation is terminal: {rendered}"
    );
}

#[tokio::test]
async fn an_unknown_id_is_distinct_from_an_unavailable_model() {
    // Three outcomes that must stay apart: served, configured-but-broken, and
    // never configured. Collapsing the last two is what makes a typo look like
    // an outage.
    let broken = other_fixture_with(|c| {
        c.model_id = "broken".to_owned();
        c.sha256 = "f".repeat(64);
    });
    let client = client_over(vec![fixture_config(), broken]).await;

    let unknown = client
        .run(&ctx(), "never-configured", tiny_inputs(2))
        .await
        .expect_err("unknown id fails");
    assert!(
        matches!(unknown, OnnxRunnerError::UnknownModel { .. }),
        "got {unknown:?}"
    );

    let unavailable = client
        .run(&ctx(), "broken", other_inputs(2))
        .await
        .expect_err("unavailable model fails");
    assert!(
        matches!(unavailable, OnnxRunnerError::ModelUnavailable { .. }),
        "got {unavailable:?}"
    );

    assert!(
        client.run(&ctx(), MODEL_ID, tiny_inputs(2)).await.is_ok(),
        "the good one still serves"
    );
}

#[tokio::test]
async fn a_contract_mismatch_on_the_second_model_names_that_model() {
    // A generic startup error would send an operator reading the wrong config
    // block. The first model is fine; the second declares an output its graph
    // does not have.
    //
    // The declared output needs a dtype as well as a name, or the config check
    // rejects it as `invalid_config` before a session is ever built — which is
    // correct behaviour, and a different failure from the one under test here.
    let wrong = other_fixture_with(|c| {
        c.expected_outputs = vec!["tokens_out".to_owned(), "not_in_the_graph".to_owned()];
        c.output_dtypes.insert(
            "not_in_the_graph".to_owned(),
            onnx_runner::config::OutputDtype::U8,
        );
    });
    let client = client_over(vec![fixture_config(), wrong]).await;
    let models = client.list_models(&ctx()).await.expect("lists");

    let tiny = models
        .iter()
        .find(|m| m.model_id == MODEL_ID)
        .expect("listed");
    let other = models
        .iter()
        .find(|m| m.model_id == OTHER_MODEL_ID)
        .expect("listed even though it failed");

    assert!(
        tiny.is_ready(),
        "one bad model does not take the other down"
    );
    assert!(!other.is_ready());

    let err = client
        .run(&ctx(), OTHER_MODEL_ID, other_inputs(2))
        .await
        .expect_err("unavailable");
    let rendered = err.to_string();
    assert!(
        rendered.contains(OTHER_MODEL_ID),
        "names which model: {rendered}"
    );
    assert!(
        rendered.contains("contract_mismatch"),
        "and why, in the stable vocabulary: {rendered}"
    );
}

#[tokio::test]
async fn tokenizers_are_per_model() {
    // Both fixtures happen to share a tokenizer file, but they must resolve it
    // through their own config rather than through a shared one.
    let client = two_model_client().await;

    let a = client
        .encode(&ctx(), MODEL_ID, "hello world")
        .await
        .expect("tiny encodes");
    let b = client
        .encode(&ctx(), OTHER_MODEL_ID, "hello world")
        .await
        .expect("other encodes");

    assert_eq!(a.ids, b.ids, "same tokenizer file, same answer");
    assert!(!a.ids.is_empty());
}

// --- Concurrency across and within models -----------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_calls_across_both_models_all_return_their_own_outputs() {
    let client = Arc::new(
        client_over(vec![
            common::fixture_with(|c| c.replicas = 2),
            other_fixture_with(|c| c.replicas = 2),
        ])
        .await,
    );

    let mut tasks = Vec::new();
    for i in 0..24_usize {
        let client = Arc::clone(&client);
        // Interleaved, and each call carries a payload unique to it, so a
        // crossed response is detectable rather than merely improbable.
        let n = 1 + i % 5;
        tasks.push(tokio::spawn(async move {
            if i % 2 == 0 {
                let out = client.run(&ctx(), MODEL_ID, tiny_inputs(n)).await?;
                let expected: Vec<i64> = sequential_ids(n);
                assert_eq!(named(&out, "ids_out").data, TensorData::I64(expected));
            } else {
                let out = client.run(&ctx(), OTHER_MODEL_ID, other_inputs(n)).await?;
                let expected: Vec<bool> = (0..n).map(|k| k % 3 == 0).collect();
                assert_eq!(named(&out, "marks_out").data, TensorData::Bool(expected));
            }
            Ok::<_, OnnxRunnerError>(())
        }));
    }

    for task in tasks {
        task.await
            .expect("no task panicked")
            .expect("every call succeeds");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slow_model_does_not_block_the_other() {
    // Per-model replica pools mean the two must not share a queue. Saturating
    // one model's single replica must leave the other's calls answerable.
    //
    // Asserted by completion, not by timing: a wall-clock threshold on a
    // fixture this small measures scheduler noise. If the pools were shared,
    // `max_in_flight` on the saturated model would shed the other model's
    // calls as `Overloaded` — which is what this would catch.
    let client = Arc::new(
        client_over(vec![
            common::fixture_with(|c| {
                c.replicas = 1;
                c.max_in_flight = Some(1);
            }),
            other_fixture_config(),
        ])
        .await,
    );

    let saturate: Vec<_> = (0..8)
        .map(|_| {
            let client = Arc::clone(&client);
            tokio::spawn(async move { client.run(&ctx(), MODEL_ID, tiny_inputs(64)).await })
        })
        .collect();

    let other = client
        .run(&ctx(), OTHER_MODEL_ID, other_inputs(4))
        .await
        .expect("the other model answers while tiny is saturated");
    assert_eq!(named(&other, "tokens_out").shape, vec![1, 4]);

    for task in saturate {
        // Some may shed as `Overloaded`: that is the point of the ceiling.
        // What must not happen is a panic or a wrong-model answer.
        drop(task.await.expect("no task panicked"));
    }
}

// --- The numbers the acceptance criteria ask to be recorded ------------------

#[tokio::test]
async fn records_two_model_startup_cost() {
    // Recorded, not asserted: a threshold here would be a flake on shared CI,
    // and the figure that matters to an operator comes from the benchmark
    // harness against real artifacts. This is the fixture-scale floor.
    let before = resident_kb();
    let started = Instant::now();
    let client = two_model_client().await;
    let elapsed = started.elapsed();
    let after = resident_kb();

    let models = client.list_models(&ctx()).await.expect("lists");
    assert_eq!(models.len(), 2);

    println!("two-model startup: {} ms", elapsed.as_millis());
    match (before, after) {
        (Some(b), Some(a)) => println!(
            "resident: {b} kB -> {a} kB (delta {} kB)",
            a.saturating_sub(b)
        ),
        _ => println!("resident: unavailable on this platform"),
    }
}
