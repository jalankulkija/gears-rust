//! The ONLY file in this gear that imports `tokenizers`.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use tokenizers::Tokenizer;

use onnx_runner_sdk::{OnnxRunnerError, TokenEncoding};

use crate::domain::metrics::{InferenceOutcome, NoopMetrics, OnnxRunnerMetricsPort};

/// Tokenization is lighter than inference but not free — large inputs and
/// `onig` regex work both cost real CPU — so it runs on the blocking pool for
/// the same reason inference does. `Tokenizer::encode` takes `&self`, so
/// unlike a session no exclusion is needed: an `Arc` clone moves into the
/// blocking task and any number of encodes proceed concurrently.
pub struct LoadedTokenizer {
    tokenizer: Arc<Tokenizer>,
    metrics: Arc<dyn OnnxRunnerMetricsPort>,
}

impl LoadedTokenizer {
    /// Loads a `tokenizer.json` from disk.
    ///
    /// # Errors
    ///
    /// Returns [`OnnxRunnerError::Internal`] if the file cannot be read or
    /// parsed.
    pub fn load(path: &Path) -> Result<Self, OnnxRunnerError> {
        let tokenizer = Tokenizer::from_file(path).map_err(|e| {
            OnnxRunnerError::Internal(format!("failed to load tokenizer {}: {e}", path.display()))
        })?;
        Ok(Self {
            tokenizer: Arc::new(tokenizer),
            metrics: Arc::new(NoopMetrics),
        })
    }

    /// Attaches the metrics port. Called once at gear init.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<dyn OnnxRunnerMetricsPort>) -> Self {
        self.metrics = metrics;
        self
    }

    /// Runs `f` on the blocking pool with a handle to the tokenizer.
    ///
    /// Cancel-safe: dropping the returned future discards the result and
    /// leaves nothing to clean up, since the tokenizer is shared rather than
    /// taken.
    async fn with_tokenizer<T, F>(&self, model_id: &str, f: F) -> Result<T, OnnxRunnerError>
    where
        F: FnOnce(&Tokenizer) -> Result<T, OnnxRunnerError> + Send + 'static,
        T: Send + 'static,
    {
        let tokenizer = Arc::clone(&self.tokenizer);
        let owned_id = model_id.to_owned();
        let started = Instant::now();

        let result = tokio::task::spawn_blocking(move || f(&tokenizer))
            .await
            .map_err(|join_err| {
                OnnxRunnerError::Internal(format!(
                    "tokenization task for model {owned_id} did not complete: {join_err}"
                ))
            })
            .and_then(|inner| inner);

        let outcome = match &result {
            Ok(_) => InferenceOutcome::Ok,
            Err(e) => e.into(),
        };
        self.metrics
            .record_tokenize(model_id, outcome, started.elapsed());
        result
    }

    /// Encodes `text`, returning ids/attention-mask/special-tokens-mask plus
    /// offsets into the original `text`.
    ///
    /// **Offsets are byte offsets, and this is verified**, not assumed:
    /// `tests/tokenizer_offset_tests.rs` slices the original text by every
    /// returned offset pair and asserts it reproduces that token, across
    /// Cyrillic, CJK and combining sequences. Measured: `привет` is 6
    /// characters and 12 bytes, and returns `(0, 12)`.
    ///
    /// This mattered because the wrong implementation fails quietly. A
    /// character offset used as a byte offset still slices — into a shorter,
    /// wrong span — so every consumer mapping a span back to source text would
    /// have been silently wrong for non-ASCII input. Note that this is *not*
    /// the same data path as `GLiNER2`'s own Python-side output layer, which
    /// dlp-bench separately found to be character-offset; do not generalize
    /// this result to that one.
    /// # Errors
    ///
    /// Returns [`OnnxRunnerError::TokenizationFailed`] if the tokenizer
    /// rejects the input, or [`OnnxRunnerError::Internal`] if the blocking
    /// task does not complete.
    #[tracing::instrument(
        name = "onnx_runner.encode",
        skip_all,
        fields(model_id = %model_id, text_bytes = text.len())
    )]
    pub async fn encode(
        &self,
        model_id: &str,
        text: String,
    ) -> Result<TokenEncoding, OnnxRunnerError> {
        // `skip_all` plus an explicit byte count: the length is useful, the
        // text is confidential and attacker-controlled.
        let owned_id = model_id.to_owned();
        self.with_tokenizer(model_id, move |tokenizer| {
            encode_to_token_encoding(tokenizer, &owned_id, &text)
        })
        .await
    }

    /// Tokenize `fragment` to raw sub-token ids with `add_special_tokens =
    /// false` — see `OnnxRunnerClientV1::encode_fragment`'s docs for why this
    /// is a distinct primitive from [`Self::encode`].
    ///
    /// # Errors
    ///
    /// Returns [`OnnxRunnerError::TokenizationFailed`] if the tokenizer
    /// rejects the input, or [`OnnxRunnerError::Internal`] if the blocking
    /// task does not complete.
    #[tracing::instrument(
        name = "onnx_runner.encode_fragment",
        skip_all,
        fields(model_id = %model_id, text_bytes = fragment.len())
    )]
    pub async fn encode_fragment(
        &self,
        model_id: &str,
        fragment: String,
    ) -> Result<Vec<i64>, OnnxRunnerError> {
        let owned_id = model_id.to_owned();
        self.with_tokenizer(model_id, move |tokenizer| {
            let encoding = tokenizer.encode(fragment, false).map_err(|e| {
                OnnxRunnerError::TokenizationFailed {
                    model_id: owned_id,
                    message: e.to_string(),
                }
            })?;
            Ok(encoding.get_ids().iter().map(|&id| i64::from(id)).collect())
        })
        .await
    }
}

fn encode_to_token_encoding(
    tokenizer: &Tokenizer,
    model_id: &str,
    text: &str,
) -> Result<TokenEncoding, OnnxRunnerError> {
    let encoding =
        tokenizer
            .encode(text, true)
            .map_err(|e| OnnxRunnerError::TokenizationFailed {
                model_id: model_id.to_owned(),
                message: e.to_string(),
            })?;

    let ids: Vec<i64> = encoding.get_ids().iter().map(|&id| i64::from(id)).collect();
    let attention_mask: Vec<i64> = encoding
        .get_attention_mask()
        .iter()
        .map(|&m| i64::from(m))
        .collect();
    let special_tokens_mask: Vec<bool> = encoding
        .get_special_tokens_mask()
        .iter()
        .map(|&m| m == 1)
        .collect();
    // `TokenEncoding` carries offsets as u32. The gear caps text length
    // well below u32::MAX, but that cap is operator-set, so the conversion
    // is checked rather than assumed.
    let offsets: Vec<(u32, u32)> = encoding
        .get_offsets()
        .iter()
        .map(|&(start, end)| {
            let to_u32 = |v: usize| {
                u32::try_from(v).map_err(|_| OnnxRunnerError::TokenizationFailed {
                    model_id: model_id.to_owned(),
                    message: format!("byte offset {v} exceeds the u32 offset range"),
                })
            };
            Ok((to_u32(start)?, to_u32(end)?))
        })
        .collect::<Result<_, OnnxRunnerError>>()?;

    Ok(TokenEncoding {
        ids,
        attention_mask,
        offsets,
        special_tokens_mask,
    })
}
