//! `GLiNER2` prompt construction, ported from `dlp-bench`'s
//! `src/detectors/gliner2/prompt.rs`, adapted to tokenize via
//! `OnnxRunnerClientV1::encode_fragment` (async, in-process call) instead of a
//! locally-held `tokenizers::Tokenizer`.
//!
//! The encoder input is *not* a plain tokenization of the text. It is a
//! schema prompt followed by the text, with no `[CLS]`/`[SEP]`:
//!
//! ```text
//! (  [P]  entities  (  [E] label0  [E] label1 ... )  )  [SEP_TEXT]  w0 w1 ...
//! ```
//!
//! Each *word* in that sequence is tokenized independently with
//! `add_special_tokens = false` and the sub-token id lists are
//! concatenated. Two asymmetries from the reference that are easy to get
//! backwards:
//!
//! * schema label strings are **not** lowercased;
//! * text words **are** lowercased.
//!
//! `schema_special_indices` is `[position of [P]] + [position of each
//! [E]]`, hence its length is `num_labels + 1` — the graph reads the `[P]`
//! embedding for the count head and the `[E]` embeddings as label
//! representations.
//!
//! # Why the prefix is precomputed
//!
//! Every token position in the schema section depends only on preceding
//! schema tokens, so the prefix ids and `schema_special_indices` are
//! text-independent and are built once at gear init. The graph consumes
//! raw ids plus index tensors and never re-derives the schema, so this is
//! exact, not an approximation.

use std::collections::HashMap;

use onnx_runner_sdk::OnnxRunnerClientV1;
use prompt_scanner_sdk::PromptScannerError;
use toolkit_security::SecurityContext;

use super::words::Word;

/// Special token ids, verified against the shipped `tokenizer.json` at
/// load — see `Gliner2Backend::load`.
pub mod special {
    pub const SEP_TEXT: &str = "[SEP_TEXT]";
    pub const P: &str = "[P]";
    pub const E: &str = "[E]";

    pub const SEP_TEXT_ID: i64 = 250_103;
    pub const P_ID: i64 = 250_104;
    pub const E_ID: i64 = 250_106;
}

/// The precomputed, text-independent part of the encoder input.
#[derive(Debug, Clone)]
pub struct PromptPrefix {
    /// Token ids for `( [P] entities ( [E] L0 ... ) ) [SEP_TEXT]`.
    pub ids: Vec<i64>,
    /// `[pos([P])] + [pos([E]_i) for each label]`; length `num_labels + 1`.
    pub schema_special_indices: Vec<i64>,
    pub num_labels: usize,
}

impl PromptPrefix {
    pub fn len(&self) -> usize {
        self.ids.len()
    }
}

/// Caches sub-token ids per lowercased word. Unbounded reset (not LRU) once
/// `cap` is hit — correctness does not depend on eviction policy because
/// encoding is pure, so this is just a bounded-memory cache.
#[derive(Debug, Default)]
pub struct WordIdCache {
    map: HashMap<Box<str>, Vec<i64>>,
    cap: usize,
}

impl WordIdCache {
    pub fn new(cap: usize) -> Self {
        Self {
            map: HashMap::new(),
            cap,
        }
    }

    async fn get_or_insert(
        &mut self,
        word: &str,
        ctx: &SecurityContext,
        onnx_client: &dyn OnnxRunnerClientV1,
        model_id: &str,
    ) -> Result<Vec<i64>, PromptScannerError> {
        if let Some(ids) = self.map.get(word) {
            return Ok(ids.clone());
        }
        if self.map.len() >= self.cap {
            self.map.clear(); // cheap bounded reset; hit rate recovers immediately
        }
        let ids = encode_fragment(ctx, onnx_client, model_id, word).await?;
        self.map.insert(word.into(), ids.clone());
        Ok(ids)
    }
}

/// Tokenize one word to ids, with no special tokens added — the exact
/// analogue of the reference's `convert_tokens_to_ids(tokenizer.tokenize(word))`.
pub async fn encode_fragment(
    ctx: &SecurityContext,
    onnx_client: &dyn OnnxRunnerClientV1,
    model_id: &str,
    word: &str,
) -> Result<Vec<i64>, PromptScannerError> {
    onnx_client
        .encode_fragment(ctx, model_id, word)
        .await
        .map_err(|e| PromptScannerError::BackendUnavailable {
            backend: "onnx-scanner".to_owned(),
            reason: e.to_string(),
        })
}

/// Current length of `ids` as an `i64` position — sequence lengths here are
/// bounded by a schema's label count times a few tokens each, nowhere near
/// `i64::MAX`.
#[allow(clippy::cast_possible_wrap)]
fn index_of(ids: &[i64]) -> i64 {
    ids.len() as i64
}

async fn push_word(
    ids: &mut Vec<i64>,
    ctx: &SecurityContext,
    onnx_client: &dyn OnnxRunnerClientV1,
    model_id: &str,
    word: &str,
) -> Result<(), PromptScannerError> {
    ids.extend(encode_fragment(ctx, onnx_client, model_id, word).await?);
    Ok(())
}

/// Build the schema prefix for a fixed label set.
pub async fn build_prefix(
    ctx: &SecurityContext,
    onnx_client: &dyn OnnxRunnerClientV1,
    model_id: &str,
    labels: &[&str],
) -> Result<PromptPrefix, PromptScannerError> {
    if labels.is_empty() {
        return Err(PromptScannerError::validation(
            "`GLiNER2` schema must have at least one label",
        ));
    }

    // Word sequence, mirroring `_transform_schema` +
    // `_format_input_with_mapping`. Note the single schema means the
    // `[SEP_STRUCT]` the reference appends is immediately popped, so it
    // never appears.
    let mut ids: Vec<i64> = Vec::with_capacity(labels.len() * 4 + 8);
    let mut schema_special_indices: Vec<i64> = Vec::with_capacity(labels.len() + 1);

    push_word(&mut ids, ctx, onnx_client, model_id, "(").await?;
    // `[P]` marks the parent; its embedding feeds the count head.
    schema_special_indices.push(index_of(&ids));
    push_word(&mut ids, ctx, onnx_client, model_id, special::P).await?;
    push_word(&mut ids, ctx, onnx_client, model_id, "entities").await?;
    push_word(&mut ids, ctx, onnx_client, model_id, "(").await?;
    for label in labels {
        // `[E]` marks a child field; its embedding is that label's query vector.
        schema_special_indices.push(index_of(&ids));
        push_word(&mut ids, ctx, onnx_client, model_id, special::E).await?;
        // Label strings keep their case — the reference does not lowercase them.
        push_word(&mut ids, ctx, onnx_client, model_id, label).await?;
    }
    push_word(&mut ids, ctx, onnx_client, model_id, ")").await?;
    push_word(&mut ids, ctx, onnx_client, model_id, ")").await?;
    push_word(&mut ids, ctx, onnx_client, model_id, special::SEP_TEXT).await?;

    debug_assert_eq!(schema_special_indices.len(), labels.len() + 1);

    Ok(PromptPrefix {
        ids,
        schema_special_indices,
        num_labels: labels.len(),
    })
}

/// Per-document encoder input.
#[derive(Debug, Default)]
pub struct EncodedInput {
    pub input_ids: Vec<i64>,
    pub attention_mask: Vec<i64>,
    /// Absolute position of each text word's first sub-token.
    pub text_word_indices: Vec<i64>,
}

impl EncodedInput {
    pub fn seq_len(&self) -> usize {
        self.input_ids.len()
    }
}

/// Append the text words to the precomputed prefix.
pub async fn build_input(
    prefix: &PromptPrefix,
    words: &[Word],
    ctx: &SecurityContext,
    onnx_client: &dyn OnnxRunnerClientV1,
    model_id: &str,
    cache: &mut WordIdCache,
) -> Result<EncodedInput, PromptScannerError> {
    let mut out = EncodedInput {
        input_ids: prefix.ids.clone(),
        attention_mask: Vec::new(),
        text_word_indices: Vec::with_capacity(words.len()),
    };

    for w in words {
        // Recorded *before* extending, which reproduces the reference's
        // behaviour for a word that tokenizes to nothing: it inherits the
        // next word's first-subtoken position.
        out.text_word_indices.push(index_of(&out.input_ids));
        let sub = cache
            .get_or_insert(&w.lower, ctx, onnx_client, model_id)
            .await?;
        out.input_ids.extend_from_slice(&sub);
    }

    if out.input_ids.is_empty() {
        return Err(PromptScannerError::internal(
            "onnx-scanner: empty gliner2 encoder input",
        ));
    }

    // The reference has a latent out-of-bounds here: a *trailing*
    // zero-subtoken word records an index equal to `seq_len`, and the
    // gather then reads past the end. Clamp rather than reproduce a crash.
    let last = index_of(&out.input_ids) - 1;
    for v in &mut out.text_word_indices {
        if *v > last {
            *v = last;
        }
    }

    out.attention_mask = vec![1; out.input_ids.len()];
    Ok(out)
}

/// Load-time sanity check: a tokenizer whose special-token ids differ is
/// not this model's tokenizer, and every index we compute would be
/// silently wrong.
pub async fn assert_special_tokens(
    ctx: &SecurityContext,
    onnx_client: &dyn OnnxRunnerClientV1,
    model_id: &str,
) -> Result<(), PromptScannerError> {
    for (name, want) in [
        (special::P, special::P_ID),
        (special::E, special::E_ID),
        (special::SEP_TEXT, special::SEP_TEXT_ID),
    ] {
        let ids = encode_fragment(ctx, onnx_client, model_id, name).await?;
        if ids != vec![want] {
            return Err(PromptScannerError::internal(format!(
                "onnx-scanner: special token {name} encodes to {ids:?}, expected [{want}]; \
                 this is not the tokenizer gliner2 was built against"
            )));
        }
    }
    Ok(())
}
