//! onnx-runner SDK
//!
//! Public API for the `onnx-runner` gear — a generic ONNX model execution
//! service (tensor in/out + tokenization only, no model-family-specific
//! logic). See `gears/onnx-runner/docs/DESIGN.md` for the full design.

#![forbid(unsafe_code)]

pub mod client;
pub mod errors;
pub mod models;
pub mod tensors;

pub use client::OnnxRunnerClientV1;
pub use errors::OnnxRunnerError;
pub use models::{
    LoadedModelInfo, ModelFailureReason, ModelInfo, ModelState, NamedTensor, TensorData,
    TokenEncoding,
};
pub use tensors::token_classifier_inputs;
