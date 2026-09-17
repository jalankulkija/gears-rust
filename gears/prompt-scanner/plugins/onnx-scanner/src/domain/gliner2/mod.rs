//! Fastino `GLiNER2` PII detector — a zero-shot span model specialized for
//! PII, replacing the BIO token classifier as `onnx-scanner`'s default
//! backend for the `GenAI` Security demo (see
//! `gears/prompt-scanner/docs/POLICY_PROVIDER_AND_DEMO_CONSOLE.md`).
//!
//! Ported from the `dlp-bench` detector-benchmark repo's
//! `src/detectors/gliner2/` (a from-scratch, tested Rust port of the
//! reference `GLiNER2` `PyTorch` inference path), adapted to run its ONNX graph
//! through `onnx-runner`'s generic `OnnxRunnerClientV1::run`/`encode_fragment`
//! instead of holding a private `ort` session — `onnx-runner` stays the
//! only gear that owns ONNX Runtime resources.
//!
//! # Provenance and its caveats
//!
//! The export (`yethdev/gliner2-pii-onnx`) is community-produced and
//! unaffiliated with Fastino. Its card claims span-set parity with the
//! `PyTorch` engine at thresholds 0.3/0.5/0.7, but `dlp-bench`'s own
//! measurements are the only ones this integration relies on — see that
//! repo's `REPORT.md`.
//!
//! # Structure
//!
//! * [`tensor`] — adapts `onnx_runner_sdk::NamedTensor` to the plain
//!   `(shape, data)` view `decode` needs.
//! * [`words`] — text preparation and the reference word splitter (ported
//!   verbatim, self-contained, no `ort`/tokenizer dependency).
//! * [`prompt`] — schema prefix and per-document encoder input, built via
//!   `OnnxRunnerClientV1::encode_fragment`.
//! * [`decode`] — span extraction and per-label suppression.

pub mod decode;
pub mod prompt;
pub mod tensor;
pub mod words;

use std::sync::Arc;

use async_trait::async_trait;
use onnx_runner_sdk::{NamedTensor, OnnxRunnerClientV1, TensorData};
use prompt_scanner_sdk::PromptScannerError;
use tokio::sync::Mutex;
use toolkit_macros::domain_model;
use toolkit_security::SecurityContext;

use self::prompt::{EncodedInput, PromptPrefix, WordIdCache};
use crate::domain::backend::PiiBackend;
use crate::domain::entity::Entity;

const EXPECTED_INPUTS: &[&str] = &[
    "input_ids",
    "attention_mask",
    "text_word_indices",
    "schema_special_indices",
];
const EXPECTED_OUTPUTS: &[&str] = &["span_probs", "count_logits"];

/// The model's span-width axis. Asserted against the first real output.
const EXPECTED_MAX_WIDTH: usize = 8;

#[domain_model]
pub struct Gliner2Backend {
    onnx_client: Arc<dyn OnnxRunnerClientV1>,
    model_id: String,
    labels: Vec<String>,
    /// Spans scoring below this are never emitted, independent of
    /// `Service`'s separate redact/warn action thresholds (which operate
    /// on whatever confidence a backend does emit) — see the model card's
    /// 0.3/0.5/0.7 operating points.
    decode_threshold: f32,
    prefix: PromptPrefix,
    /// `Mutex` because `WordIdCache` mutates per call while `PiiBackend`
    /// only gives `&self` — same pattern `bio_backend`'s upstream `Service`
    /// avoids needing entirely by being stateless; here the cache is the
    /// one piece of real per-backend state.
    cache: Mutex<WordIdCache>,
}

impl Gliner2Backend {
    /// Precomputes the schema prefix (via `onnx_client.encode_fragment`, so
    /// this is async and must run once at gear `init()`, not per scan —
    /// see `PromptPrefix`'s docs on why it's safe to reuse across
    /// documents).
    ///
    /// # Errors
    ///
    /// Returns [`PromptScannerError`] if `onnx-runner` is unreachable, the
    /// configured `model_id` isn't loaded there, or the tokenizer's
    /// special-token ids don't match what this backend expects (wrong
    /// tokenizer for this model).
    pub async fn load(
        onnx_client: Arc<dyn OnnxRunnerClientV1>,
        model_id: String,
        labels: Vec<String>,
        decode_threshold: f32,
    ) -> Result<Self, PromptScannerError> {
        let ctx = bootstrap_context();
        prompt::assert_special_tokens(&ctx, onnx_client.as_ref(), &model_id).await?;

        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let prefix =
            prompt::build_prefix(&ctx, onnx_client.as_ref(), &model_id, &label_refs).await?;

        Ok(Self {
            onnx_client,
            model_id,
            labels,
            decode_threshold,
            prefix,
            cache: Mutex::new(WordIdCache::new(50_000)),
        })
    }

    /// Prefix length in tokens — the fixed per-call cost imposed by the
    /// schema (dominant term in short-prompt latency; see the module docs
    /// this was ported from).
    pub fn prefix_tokens(&self) -> usize {
        self.prefix.len()
    }
}

/// `onnx-runner` ignores `SecurityContext` entirely for `run`/`encode*`
/// (see `OnnxRunnerLocalClient` — it's generic tensor execution, not a
/// tenant-scoped resource), so a fixed bootstrap context is fine for the
/// load-time prefix build; there is no real caller identity to thread
/// through at gear `init()` anyway.
fn bootstrap_context() -> SecurityContext {
    SecurityContext::builder()
        .subject_id(uuid::Uuid::nil())
        .subject_tenant_id(uuid::Uuid::nil())
        .build()
        .unwrap_or_else(|_| unreachable!("subject_id and subject_tenant_id are always set above"))
}

#[async_trait]
impl PiiBackend for Gliner2Backend {
    async fn detect_entities(
        &self,
        ctx: &SecurityContext,
        text: &str,
    ) -> Result<Vec<Entity>, PromptScannerError> {
        let prepared = words::prepare(text);
        if prepared.words.is_empty() {
            return Ok(Vec::new());
        }

        let enc: EncodedInput = {
            let mut cache = self.cache.lock().await;
            prompt::build_input(
                &self.prefix,
                &prepared.words,
                ctx,
                self.onnx_client.as_ref(),
                &self.model_id,
                &mut cache,
            )
            .await?
        };

        let seq_len = enc.seq_len();
        let inputs = vec![
            NamedTensor::new(
                "input_ids",
                vec![1, seq_len],
                TensorData::I64(enc.input_ids),
            ),
            NamedTensor::new(
                "attention_mask",
                vec![1, seq_len],
                TensorData::I64(enc.attention_mask),
            ),
            NamedTensor::new(
                "text_word_indices",
                vec![enc.text_word_indices.len()],
                TensorData::I64(enc.text_word_indices),
            ),
            NamedTensor::new(
                "schema_special_indices",
                vec![self.prefix.schema_special_indices.len()],
                TensorData::I64(self.prefix.schema_special_indices.clone()),
            ),
        ];
        debug_assert!(
            EXPECTED_INPUTS
                .iter()
                .all(|n| inputs.iter().any(|t| t.name == *n))
        );

        let outputs = self
            .onnx_client
            .run(ctx, &self.model_id, inputs)
            .await
            .map_err(|e| PromptScannerError::BackendUnavailable {
                backend: "onnx-scanner".to_owned(),
                reason: e.to_string(),
            })?;

        let span_probs = tensor::f32_tensor(tensor::find_tensor(&outputs, EXPECTED_OUTPUTS[0])?)?;
        let count_logits = tensor::f32_tensor(tensor::find_tensor(&outputs, EXPECTED_OUTPUTS[1])?)?;

        let dims = decode::SpanDims::from_tensor(&span_probs).ok_or_else(|| {
            PromptScannerError::internal(format!(
                "onnx-scanner: span_probs must be rank 3, got shape {:?}",
                span_probs.shape
            ))
        })?;
        if dims.max_width != EXPECTED_MAX_WIDTH {
            return Err(PromptScannerError::internal(format!(
                "onnx-scanner: expected span width axis {EXPECTED_MAX_WIDTH}, got {} (shape {:?})",
                dims.max_width, span_probs.shape
            )));
        }
        if dims.num_labels != self.prefix.num_labels {
            return Err(PromptScannerError::internal(format!(
                "onnx-scanner: graph returned {} labels but {} were requested",
                dims.num_labels, self.prefix.num_labels
            )));
        }

        let raw = decode::decode(
            &span_probs,
            dims,
            &count_logits,
            &prepared.words,
            &prepared.text,
            self.decode_threshold,
        );

        // Offsets are into the prepared text, which may carry a synthetic
        // trailing '.'. Clip to the caller's text so a returned span is
        // always sliceable against what they passed in.
        let limit = prepared.original_len();
        let mut out = Vec::with_capacity(raw.len());
        for s in raw {
            let Some(category) = self.labels.get(s.label_idx) else {
                return Err(PromptScannerError::internal(format!(
                    "onnx-scanner: decoded label index {} is outside the {}-label schema",
                    s.label_idx,
                    self.labels.len()
                )));
            };
            let end = s.end.min(limit);
            if s.start >= end {
                continue; // the span was entirely the synthetic '.'
            }
            #[allow(clippy::cast_possible_truncation)] // payload sizes are bounded well under u32
            out.push(Entity {
                category: category.clone(),
                start: s.start as u32,
                end: end as u32,
                confidence: s.score,
            });
        }

        Ok(out)
    }
}
