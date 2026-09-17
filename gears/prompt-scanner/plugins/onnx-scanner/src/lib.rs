//! ONNX Scanner Plugin
//!
//! `ScannerBackendClient` implementation for `prompt-scanner` that detects
//! PII via an ONNX model, run in-process by the plugin's own model host
//! ([`runtime`]: tokenize + infer only — decoding is the backends' job) on the
//! toolkit's ONNX session. Replaces the retired `pii-plugin` and the
//! standalone `onnx-runner` gear. See `docs/runtime/DESIGN.md`.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

pub mod config;
mod domain;
/// Detection-quality evaluation tooling. Never built into the shipped plugin.
#[cfg(feature = "eval")]
pub mod eval;
/// The model host. `#[doc(hidden)] pub` only so the integration tests can drive
/// the real load, run and readiness paths. Not public API.
#[doc(hidden)]
pub mod runtime;
/// Re-exported only for `eval`, whose public scoring functions take it. The
/// shipped plugin keeps `domain` private.
#[cfg(feature = "eval")]
pub use domain::entity::Entity;
pub mod gear;

pub use gear::OnnxScanner;
