#![cfg(feature = "real-model-tests")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Proves the toolkit's bounded session open (via `runtime::load_models`) actually bounds the `ort`
//! startup hang, rather than merely documenting it.
//!
//! Needs its own test binary: `ort` caches its API pointer in a `OnceLock` and
//! reads `ORT_DYLIB_PATH` once per process, so a wedged runtime cannot be
//! reproduced in-process alongside tests that want a working one.
//!
//! Ignored by default because it needs an `ORT_DYLIB_PATH` that cannot be loaded.
//! A nonexistent path reproduces the hang reliably (and bounds it at the
//! toolkit's one minute per session):
//!
//! ```text
//! ORT_DYLIB_PATH=/nonexistent/libonnxruntime.dylib \
//!   cargo test -p cf-gears-onnx-scanner --features real-model-tests \
//!   --test model_load_timeout_tests -- --ignored --nocapture
//! ```
//!
//! Without that variable the test would pass for the wrong reason, so it
//! asserts the variable is set before doing anything.

mod common;

use common::no_metrics;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use onnx_scanner::runtime::config::ModelConfig;
use onnx_scanner::runtime::{InitError, load_models};

/// The host allows one minute per configured session. This drives a
/// single-replica model, so a wedged load must give up in appreciably less
/// than two minutes — and, crucially, must give up at all.
const EXPECTED_BOUND: Duration = Duration::from_secs(110);

fn wedged_model() -> ModelConfig {
    ModelConfig {
        model_id: "wedged-runtime-model".to_owned(),
        // Never read: the hang happens in `Session::builder()`, before the
        // path matters. Loading fails earlier than this if the runtime works,
        // which is why the test asserts on the error text too.
        model_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models/gliner2-pii"),
        model_file: "model.quantized.onnx".to_owned(),
        skip_checksum: true,
        expected_inputs: Vec::new(),
        expected_outputs: Vec::new(),
        output_dtypes: BTreeMap::new(),
        replicas: 1,
        intra_threads: 1,
        ..ModelConfig::default()
    }
}

#[tokio::test]
#[ignore = "needs ORT_DYLIB_PATH pointed at an unloadable ONNX Runtime; see module docs"]
async fn wedged_runtime_times_out_instead_of_hanging_forever() {
    let dylib = std::env::var("ORT_DYLIB_PATH").expect(
        "set ORT_DYLIB_PATH to an unloadable ONNX Runtime (see module docs); without it \
         this test would pass for the wrong reason",
    );

    let token = tokio_util::sync::CancellationToken::new();

    let started = Instant::now();
    let result = load_models(vec![wedged_model()], false, no_metrics(), &token).await;
    let elapsed = started.elapsed();

    let Err(err) = result else {
        panic!("loading against an unloadable runtime ({dylib}) must fail, not succeed");
    };

    assert!(
        elapsed < EXPECTED_BOUND,
        "loading took {elapsed:?}; the hang is no longer bounded"
    );
    assert!(
        matches!(err, InitError::RuntimeHung { .. }),
        "a wedged runtime must surface as RuntimeHung, got: {err}"
    );
    assert!(
        err.leaked_init_thread(),
        "a timed-out load abandons its thread, and must say so"
    );

    let message = err.to_string();
    assert!(
        message.contains("ORT_DYLIB_PATH"),
        "the error must point the operator at the actual cause: {message}"
    );
}

/// Shutdown must not have to wait out the full load timeout.
#[tokio::test]
#[ignore = "needs ORT_DYLIB_PATH pointed at an unloadable ONNX Runtime; see module docs"]
async fn cancellation_beats_the_timeout() {
    std::env::var("ORT_DYLIB_PATH").expect("see module docs");

    let token = tokio_util::sync::CancellationToken::new();
    let canceller = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(2)).await;
        canceller.cancel();
    });

    let started = Instant::now();
    let result = load_models(vec![wedged_model()], false, no_metrics(), &token).await;
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(30),
        "cancellation took {elapsed:?}; it must not wait out the load timeout"
    );
    assert!(
        matches!(result, Err(InitError::Cancelled)),
        "cancelled loading must surface as Cancelled"
    );
}
