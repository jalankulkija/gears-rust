#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Payload content must never reach telemetry.
//!
//! This gear handles content that is attacker-controlled and confidential by
//! construction — that is the entire point of the program it belongs to. Logs
//! and spans are exactly where such content leaks, usually via a well-meant
//! debug field added during an unrelated change.
//!
//! So this is a control, not a check: a distinctive sentinel string is pushed
//! through every path the gear has, and every captured span and event is
//! searched for it. Adding `text = %text` to any instrumented function makes
//! this suite fail. **Treat its removal or weakening as a defect.**

mod common;

use std::sync::{Arc, Mutex};

use common::{MODEL_ID, ctx, fixture_config, fixture_with, no_metrics};
use onnx_runner::gear::{OnnxRunnerLocalClient, load_models};
use onnx_runner_sdk::{NamedTensor, OnnxRunnerClientV1, TensorData};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::layer::SubscriberExt;

/// A string that cannot plausibly occur by accident, so any appearance in
/// telemetry is proof it came from the payload.
const SENTINEL: &str = "zzQUUXSENTINEL7431zz";

/// Captures every field of every event *and* span this gear emits, flattened
/// to text. Deliberately indiscriminate: a leak is a leak wherever it appears.
#[derive(Clone, Default)]
struct CapturingLayer {
    captured: Arc<Mutex<Vec<String>>>,
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CapturingLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        let mut visitor = Flatten(self.captured.clone());
        event.record(&mut visitor);
        self.captured
            .lock()
            .unwrap()
            .push(event.metadata().name().to_owned());
    }

    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        _: &tracing::Id,
        _: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = Flatten(self.captured.clone());
        attrs.record(&mut visitor);
    }

    fn on_record(
        &self,
        _: &tracing::Id,
        values: &tracing::span::Record<'_>,
        _: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = Flatten(self.captured.clone());
        values.record(&mut visitor);
    }
}

struct Flatten(Arc<Mutex<Vec<String>>>);

impl tracing::field::Visit for Flatten {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0
            .lock()
            .unwrap()
            .push(format!("{}={value:?}", field.name()));
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0
            .lock()
            .unwrap()
            .push(format!("{}={value}", field.name()));
    }
}

/// Runs `body` with telemetry captured, and returns everything recorded.
async fn capture<F, Fut>(body: F) -> Vec<String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let layer = CapturingLayer::default();
    let captured = Arc::clone(&layer.captured);
    let subscriber = tracing_subscriber::registry().with(layer);

    let guard = tracing::subscriber::set_default(subscriber);
    body().await;
    drop(guard);

    captured.lock().unwrap().clone()
}

fn assert_no_leak(captured: &[String], context: &str) {
    let leaked: Vec<&String> = captured.iter().filter(|c| c.contains(SENTINEL)).collect();
    assert!(
        leaked.is_empty(),
        "payload content reached telemetry on the {context} path: {leaked:?}\n\
         This is a security control, not a style rule — see this file's module docs."
    );
    assert!(
        !captured.is_empty(),
        "captured nothing on the {context} path; the test would pass vacuously"
    );
}

async fn client() -> OnnxRunnerLocalClient {
    let entries = load_models(
        vec![fixture_config()],
        true,
        no_metrics(),
        &CancellationToken::new(),
    )
    .await
    .expect("fixture loads");
    OnnxRunnerLocalClient::new(entries)
}

/// Tensor *names* are safe and useful in telemetry; tensor *values* are not.
fn sentinel_named_tensors() -> Vec<NamedTensor> {
    vec![
        NamedTensor::new("ids", vec![1, 2], TensorData::I64(vec![1, 2])),
        NamedTensor::new("weights", vec![1, 2], TensorData::F32(vec![0.5, 0.5])),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tokenizing_does_not_leak_the_text() {
    let captured = capture(|| async {
        let client = client().await;
        let text = format!("prefix {SENTINEL} suffix");
        drop(client.encode(&ctx(), MODEL_ID, &text).await);
        drop(client.encode_fragment(&ctx(), MODEL_ID, SENTINEL).await);
    })
    .await;

    assert_no_leak(&captured, "tokenize");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_text_does_not_leak_on_rejection() {
    let captured = capture(|| async {
        let entries = load_models(
            vec![fixture_with(|c| c.max_text_bytes = 8)],
            true,
            no_metrics(),
            &CancellationToken::new(),
        )
        .await
        .expect("loads");
        let client = OnnxRunnerLocalClient::new(entries);

        let text = format!("{SENTINEL}{SENTINEL}");
        let err = client.encode(&ctx(), MODEL_ID, &text).await.unwrap_err();
        // The error itself must not carry the text either — it is logged by
        // whoever receives it.
        assert!(
            !err.to_string().contains(SENTINEL),
            "the error leaked the text"
        );
    })
    .await;

    assert_no_leak(&captured, "oversized-text rejection");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_successful_inference_does_not_leak_tensor_values() {
    let captured = capture(|| async {
        let client = client().await;
        drop(client.run(&ctx(), MODEL_ID, sentinel_named_tensors()).await);
    })
    .await;

    assert_no_leak(&captured, "inference");
}

/// A tensor *name* is caller-supplied too, and it reaches the contract error.
/// Names are permitted in telemetry by the feature's field list — this test
/// pins that the *values* still do not appear, and doubles as a check that an
/// error path logs nothing extra.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_input_does_not_leak_tensor_values() {
    let captured = capture(|| async {
        let client = client().await;
        let bad = vec![NamedTensor::new(
            "ids",
            vec![9, 9],
            TensorData::I64(vec![4242, 4242]),
        )];
        drop(client.run(&ctx(), MODEL_ID, bad).await);
    })
    .await;

    assert_no_leak(&captured, "invalid input");
    assert!(
        !captured.iter().any(|c| c.contains("4242")),
        "tensor values must not appear in telemetry: {captured:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unavailable_model_does_not_leak() {
    let captured = capture(|| async {
        let entries = load_models(
            vec![fixture_with(|c| c.model_file = "absent.onnx".to_owned())],
            false,
            no_metrics(),
            &CancellationToken::new(),
        )
        .await
        .expect("startup stays non-fatal");
        let client = OnnxRunnerLocalClient::new(entries);

        drop(client.encode(&ctx(), MODEL_ID, SENTINEL).await);
        drop(client.run(&ctx(), MODEL_ID, sentinel_named_tensors()).await);
    })
    .await;

    assert_no_leak(&captured, "unavailable model");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_model_does_not_leak() {
    let captured = capture(|| async {
        let client = client().await;
        drop(client.encode(&ctx(), "no-such-model", SENTINEL).await);
    })
    .await;

    assert_no_leak(&captured, "unknown model");
}

/// The test has teeth only if it can actually see a leak. This proves the
/// capture layer works, by leaking on purpose into the same machinery.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_capture_layer_would_catch_a_leak() {
    let captured = capture(|| async {
        tracing::info!(text = %SENTINEL, "a deliberately careless log line");
    })
    .await;

    assert!(
        captured.iter().any(|c| c.contains(SENTINEL)),
        "the capture layer missed an obvious leak, so every other test here is vacuous"
    );
}
