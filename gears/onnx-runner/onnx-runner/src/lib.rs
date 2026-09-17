#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

// === PUBLIC API (from SDK) ===
pub use onnx_runner_sdk::{
    LoadedModelInfo, ModelInfo, NamedTensor, OnnxRunnerClientV1, OnnxRunnerError, TensorData,
    TokenEncoding, token_classifier_inputs,
};

// === MODULE DEFINITION ===
pub mod gear;
pub use gear::OnnxRunner;

// === INTERNAL MODULES ===
#[doc(hidden)]
pub mod config;
#[doc(hidden)]
pub mod domain;
#[doc(hidden)]
pub mod health;
#[doc(hidden)]
pub mod infra;

/// ONNX graph generation for test fixtures and benchmark ladders. Never part
/// of the shipped gear — see the module docs.
#[cfg(feature = "fixture-gen")]
#[doc(hidden)]
pub mod fixture;
