//! The model host `onnx-scanner` runs its detector models on.
//!
//! Formerly the `onnx-runner` gear; now part of the plugin that is its only
//! consumer. The ONNX Runtime session itself is the toolkit's
//! (`toolkit-onnx-runtime`); everything a deployment needs on top of it lives
//! here: replica pool, SHA-256 and graph-contract verification, input bounds,
//! per-inference timeout and load shedding, session rebuild after a panic,
//! tokenization, metrics and readiness. See `docs/runtime/DESIGN.md`.

mod client;
pub mod config;
mod error;
/// ONNX graph generation for test fixtures and benchmark ladders. Never part
/// of the shipped plugin — see the module docs.
#[cfg(feature = "fixture-gen")]
#[doc(hidden)]
pub mod fixture;
pub mod health;
mod inputs;
pub mod local;
mod metrics;
pub mod metrics_port;
pub mod registry;
pub mod session;
pub mod tokenizer;
mod types;

pub use client::ModelRuntime;
pub use error::RuntimeError;
pub use inputs::token_classifier_inputs;
pub use local::{InitError, LocalRuntime, ModelEntry, load_models};
pub use metrics::RuntimeMetricsMeter;
pub use session::declared_input_names;
pub use types::{
    LoadedModelInfo, ModelFailureReason, ModelInfo, ModelState, NamedTensor, TensorData,
    TokenEncoding,
};
