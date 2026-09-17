use async_trait::async_trait;

use super::error::RuntimeError;
use super::types::{ModelInfo, NamedTensor, TokenEncoding};

/// What the detector backends need from the model host: generic ONNX tensor
/// execution and `HuggingFace` tokenization, with no concept of any specific
/// model family (`GLiNER`, BIO token classifiers, ...) — that logic lives in
/// the backends. See `docs/runtime/DESIGN.md`.
///
/// An internal seam, not a platform contract: the host is built by
/// `onnx-scanner` at `init` and handed to its backends directly, so there is no
/// `ClientHub` entry, no `SecurityContext` and no authorization here. Models
/// are operator-configured at startup and addressed by id; a caller can never
/// supply a path, URL or model bytes
/// (`cpt-cf-onnx-runner-constraint-no-caller-paths`). Authorization belongs to
/// the scan that owns the tenant-facing operation, in `prompt-scanner`.
///
/// A trait rather than a concrete type so a backend is testable against a fake
/// without a model on disk.
#[async_trait]
pub trait ModelRuntime: Send + Sync {
    /// Run a configured model. `model_id` must match a `ModelInfo.model_id`
    /// from `list_models` — models are operator-configured at startup, never
    /// loaded ad hoc from a caller-supplied path.
    async fn run(
        &self,
        model_id: &str,
        inputs: Vec<NamedTensor>,
    ) -> Result<Vec<NamedTensor>, RuntimeError>;

    /// Tokenize `text` against a configured tokenizer (usually the same
    /// `model_id` — `tokenizer.json` is loaded from the model's own directory
    /// alongside its `.onnx` file).
    async fn encode(&self, model_id: &str, text: &str) -> Result<TokenEncoding, RuntimeError>;

    /// Tokenize a *fragment* to raw sub-token ids: **without** the tokenizer's
    /// special tokens (`[CLS]`/`[SEP]`/...), and without offsets or an
    /// attention mask.
    ///
    /// The distinction from [`Self::encode`] is document versus fragment, not
    /// sentence versus word. Use this for a piece destined to be spliced into
    /// a larger sequence you are assembling yourself — a zero-shot span model
    /// building a schema prefix out of label strings is the motivating case,
    /// since wrapping each label in `[CLS]`/`[SEP]` mid-sequence would be
    /// wrong.
    ///
    /// Offsets are omitted deliberately: a fragment's offsets into its own
    /// string are meaningless once it has been spliced, and an attention mask
    /// is a property of the assembled sequence, not of the piece.
    async fn encode_fragment(
        &self,
        model_id: &str,
        fragment: &str,
    ) -> Result<Vec<i64>, RuntimeError>;

    /// Every configured model and its state, including the ones that failed to
    /// load. A caller that finds no entry is looking at a typo; one that finds
    /// `Unavailable` is looking at an outage.
    async fn list_models(&self) -> Result<Vec<ModelInfo>, RuntimeError>;
}
