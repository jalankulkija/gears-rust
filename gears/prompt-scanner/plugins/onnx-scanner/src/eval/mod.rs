//! Detection-quality evaluation.
//!
//! Feature-gated (`eval`) and never part of the shipped plugin: this is
//! measurement tooling that drives the same [`crate::domain::backend::PiiBackend`]
//! implementations production uses. Scoring a purpose-written decoder instead
//! would produce numbers for code nobody runs.
//!
//! See `gears/onnx-runner/docs/benchmarks/QUALITY-EVALUATION.md`.

pub mod corpus;
pub mod redact;
pub mod runner;
pub mod scoring;
