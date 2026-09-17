//! Adapter from `onnx_runner_sdk::NamedTensor` to the plain
//! `(shape, data)` view `decode.rs` (ported from dlp-bench) expects.

use onnx_runner_sdk::{NamedTensor, TensorData};
use prompt_scanner_sdk::PromptScannerError;

/// A named F32 output tensor, flattened row-major — the only shape
/// `decode.rs` needs to know about.
pub struct TensorOut {
    pub shape: Vec<usize>,
    pub data: Vec<f32>,
}

impl TensorOut {
    /// Index of the tensor's largest value, flattened.
    ///
    /// Used for `count_logits`' argmax gate (`decode::count_gate_open`).
    #[must_use]
    pub fn argmax(&self) -> usize {
        self.data
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map_or(0, |(i, _)| i)
    }
}

pub fn f32_tensor(t: &NamedTensor) -> Result<TensorOut, PromptScannerError> {
    let TensorData::F32(data) = &t.data else {
        return Err(PromptScannerError::internal(format!(
            "onnx-scanner: expected F32 tensor for '{}', got a different dtype",
            t.name
        )));
    };
    Ok(TensorOut {
        shape: t.shape.clone(),
        data: data.clone(),
    })
}

pub fn find_tensor<'a>(
    outputs: &'a [NamedTensor],
    name: &str,
) -> Result<&'a NamedTensor, PromptScannerError> {
    outputs.iter().find(|t| t.name == name).ok_or_else(|| {
        PromptScannerError::internal(format!("onnx-scanner: model returned no '{name}' output"))
    })
}
