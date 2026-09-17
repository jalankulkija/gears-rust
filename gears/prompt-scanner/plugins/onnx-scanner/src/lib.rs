//! ONNX Scanner Plugin
//!
//! `ScannerBackendClient` implementation for `prompt-scanner` that detects
//! PII via a BIO token-classification ONNX model, resolved through
//! `onnx-runner`'s `OnnxRunnerClientV1` (tokenize + infer only — decoding
//! is this plugin's job). Replaces the retired `pii-plugin`. See
//! `gears/onnx-runner/docs/DESIGN.md`.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

pub mod config;
mod domain;
/// Detection-quality evaluation tooling. Never built into the shipped plugin.
#[cfg(feature = "eval")]
pub mod eval;
/// Re-exported only for `eval`, whose public scoring functions take it. The
/// shipped plugin keeps `domain` private.
#[cfg(feature = "eval")]
pub use domain::entity::Entity;
pub mod gear;

pub use gear::OnnxScanner;
