use async_trait::async_trait;
use toolkit_security::SecurityContext;

use crate::errors::OnnxRunnerError;
use crate::models::{ModelInfo, NamedTensor, TokenEncoding};

/// Public API for the `onnx-runner` gear: generic ONNX tensor execution and
/// `HuggingFace` tokenization. Deliberately has no concept of any specific
/// model family (`GLiNER`, BIO token classifiers, ...) — that logic lives in
/// consumers (e.g. `onnx-scanner`). See `gears/onnx-runner/docs/DESIGN.md`.
///
/// # Authorization: this gear enforces none
///
/// Every method below takes a [`SecurityContext`], and **no implementation
/// checks it**. It is carried for trace propagation and for the option of
/// enforcing later; it is not an access-control boundary. Stated here because
/// a parameter that looks enforced and is not is worse than one that is
/// honestly absent.
///
/// The reasoning: this gear owns no tenant-scoped resource. Models are
/// operator-configured at startup and addressed by id, a caller can never
/// supply a path, URL or model bytes
/// (`cpt-cf-onnx-runner-constraint-no-caller-paths`), and nothing here is
/// per-tenant. There is therefore nothing for an authorization decision to be
/// *about*. Access control belongs to the consumer that owns the
/// tenant-facing operation — the scan, the search, the classification — which
/// is where the tenant, the resource and the policy actually live.
///
/// **What this means for a caller.** Reaching this client means you are
/// already inside the trust boundary: it is an in-process compute primitive
/// resolved through `ClientHub`, in the same category as `authn-resolver` or
/// `tenant-resolver`. Do not treat a successful call as evidence that the
/// subject in `ctx` was permitted anything.
///
/// **If this ever needs to change** — per-tenant model restrictions being the
/// obvious trigger — enforcement is a feature with its own design, not a
/// patch: it needs a policy source this gear does not have. Removing `ctx`
/// instead is equally a versioned change, `OnnxRunnerClientV1V2` rather than
/// an edit to this trait
/// (`cpt-cf-onnx-runner-contract-client-v1`).
///
/// Recorded in `gears/onnx-runner/docs/ADR/0011-cpt-cf-onnx-runner-adr-security-context-position.md`.
#[async_trait]
pub trait OnnxRunnerClientV1: Send + Sync {
    /// Run a configured model. `model_id` must match a `ModelInfo.model_id`
    /// from `list_models` — models are operator-configured at gear startup,
    /// never loaded ad hoc from a caller-supplied path.
    async fn run(
        &self,
        ctx: &SecurityContext,
        model_id: &str,
        inputs: Vec<NamedTensor>,
    ) -> Result<Vec<NamedTensor>, OnnxRunnerError>;

    /// Tokenize `text` against a configured tokenizer (usually the same
    /// `model_id` — the gear loads `tokenizer.json` from the model's own
    /// directory alongside its `.onnx` file).
    async fn encode(
        &self,
        ctx: &SecurityContext,
        model_id: &str,
        text: &str,
    ) -> Result<TokenEncoding, OnnxRunnerError>;

    /// Tokenize a *fragment* to raw sub-token ids: **without** the
    /// tokenizer's special tokens (`[CLS]`/`[SEP]`/...), and without offsets
    /// or an attention mask.
    ///
    /// The distinction from [`Self::encode`] is document versus fragment, not
    /// sentence versus word. Use this for a piece destined to be spliced into
    /// a larger sequence you are assembling yourself — a zero-shot span model
    /// building a schema prefix out of label strings is the motivating case,
    /// since wrapping each label in `[CLS]`/`[SEP]` mid-sequence would be
    /// wrong.
    ///
    /// Offsets are omitted deliberately rather than forgotten: a fragment's
    /// offsets into its own string are meaningless once it has been spliced,
    /// and an attention mask is a property of the assembled sequence, not of
    /// the piece.
    ///
    /// Still model-family-agnostic: this is "tokenize with
    /// `add_special_tokens = false`", not a GLiNER-specific concept.
    async fn encode_fragment(
        &self,
        ctx: &SecurityContext,
        model_id: &str,
        fragment: &str,
    ) -> Result<Vec<i64>, OnnxRunnerError>;

    async fn list_models(&self, ctx: &SecurityContext) -> Result<Vec<ModelInfo>, OnnxRunnerError>;
}
