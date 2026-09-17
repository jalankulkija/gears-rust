//! [`PiiBackend`] implementation for a plain BIO token-classification model
//! (`minilm-l6-pii` and similar) — see `gears/onnx-runner/docs/DESIGN.md`'s
//! "Which PII model" decision. Calls `OnnxRunnerClientV1` for tokenization
//! and inference; BIO-specific decoding lives in
//! `domain::bio_token_classifier`.

use std::sync::Arc;

use async_trait::async_trait;
use onnx_runner_sdk::{OnnxRunnerClientV1, TensorData, token_classifier_inputs};
use prompt_scanner_sdk::PromptScannerError;
use toolkit_macros::domain_model;
use toolkit_security::SecurityContext;

use crate::domain::backend::PiiBackend;
use crate::domain::bio_token_classifier::{argmax_predictions, merge_bio};
use crate::domain::entity::Entity;

#[domain_model]
pub struct BioTokenClassifierBackend {
    /// Resolved on first use; see [`Self::declared_inputs`].
    input_names: tokio::sync::OnceCell<Vec<String>>,
    onnx_client: Arc<dyn OnnxRunnerClientV1>,
    model_id: String,
    labels: Vec<String>,
}

impl BioTokenClassifierBackend {
    #[must_use]
    pub fn new(
        onnx_client: Arc<dyn OnnxRunnerClientV1>,
        model_id: String,
        labels: Vec<String>,
    ) -> Self {
        Self {
            input_names: tokio::sync::OnceCell::new(),
            onnx_client,
            model_id,
            labels,
        }
    }

    /// The graph's declared input names, resolved once and cached.
    ///
    /// Read from `list_models` rather than configured, so it cannot drift from
    /// the graph. Cached because it is fixed for the life of a loaded model and
    /// this is on the request path.
    async fn declared_inputs(
        &self,
        ctx: &SecurityContext,
    ) -> Result<&[String], PromptScannerError> {
        let names = self
            .input_names
            .get_or_try_init(|| async {
                let models = self
                    .onnx_client
                    .list_models(ctx)
                    .await
                    .map_err(|e| onnx_error(&e))?;
                models
                    .into_iter()
                    .find(|m| m.model_id == self.model_id)
                    .and_then(|m| m.loaded)
                    .map(|l| l.input_names)
                    .ok_or_else(|| {
                        PromptScannerError::internal(format!(
                            "onnx-scanner: model '{}' is not loaded",
                            self.model_id
                        ))
                    })
            })
            .await?;
        Ok(names)
    }
}

#[async_trait]
impl PiiBackend for BioTokenClassifierBackend {
    async fn detect_entities(
        &self,
        ctx: &SecurityContext,
        text: &str,
    ) -> Result<Vec<Entity>, PromptScannerError> {
        let encoding = self
            .onnx_client
            .encode(ctx, &self.model_id, text)
            .await
            .map_err(|e| onnx_error(&e))?;

        // Built from what this graph declares, not from a fixed list.
        // `token_type_ids` is declared by some BERT token-classification
        // exports and not others — `minilm-l6-pii` has it, `piiranha-v1` and
        // `multilang-pii-ner` do not — and both omitting a declared input and
        // supplying an undeclared one are hard errors. Hardcoding the set made
        // this backend silently specific to one model.
        let seq_len = encoding.ids.len();
        let declared = self.declared_inputs(ctx).await?;
        let inputs = token_classifier_inputs(&self.model_id, &encoding, declared)
            .map_err(|e| onnx_error(&e))?;

        let outputs = self
            .onnx_client
            .run(ctx, &self.model_id, inputs)
            .await
            .map_err(|e| onnx_error(&e))?;

        let logits = outputs.iter().find(|t| t.name == "logits").ok_or_else(|| {
            PromptScannerError::internal("onnx-scanner: model returned no 'logits' output")
        })?;
        let TensorData::F32(logits) = &logits.data else {
            return Err(PromptScannerError::internal(
                "onnx-scanner: 'logits' output was not F32",
            ));
        };

        let predictions = argmax_predictions(logits, seq_len, &self.labels);
        Ok(merge_bio(
            &predictions,
            &encoding.offsets,
            &encoding.special_tokens_mask,
        ))
    }
}

fn onnx_error(e: &onnx_runner_sdk::OnnxRunnerError) -> PromptScannerError {
    PromptScannerError::BackendUnavailable {
        backend: "onnx-scanner".to_owned(),
        reason: e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use onnx_runner_sdk::{ModelInfo, OnnxRunnerError, TokenEncoding};
    use uuid::Uuid;

    use super::*;
    use onnx_runner_sdk::NamedTensor;

    fn ctx() -> SecurityContext {
        SecurityContext::builder()
            .subject_id(Uuid::new_v4())
            .subject_tenant_id(Uuid::new_v4())
            .build()
            .expect("valid SecurityContext")
    }

    fn labels() -> Vec<String> {
        ["O", "B-EMAIL", "I-EMAIL"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    }

    /// Fake `onnx-runner` client: `encode` returns one token per
    /// whitespace-separated word (byte offsets computed from the real
    /// payload), `run` returns fixed logits favoring `B-EMAIL` for the
    /// first token and `O` for the rest — enough to exercise the full
    /// tokenize -> infer -> decode -> Entity pipeline without a real
    /// model.
    struct FakeOnnxRunner;

    #[async_trait]
    impl OnnxRunnerClientV1 for FakeOnnxRunner {
        async fn run(
            &self,
            _ctx: &SecurityContext,
            _model_id: &str,
            inputs: Vec<NamedTensor>,
        ) -> Result<Vec<NamedTensor>, OnnxRunnerError> {
            let seq_len = inputs[0].shape[1];
            // Every token strongly favors "O" (index 0)...
            let mut logits = vec![0.1f32; seq_len * 3];
            for i in 0..seq_len {
                logits[i * 3] = 5.0;
            }
            // ...except the first, which strongly favors B-EMAIL (index 1).
            logits[0] = 0.1;
            logits[1] = 5.0;
            Ok(vec![NamedTensor::new(
                "logits",
                vec![seq_len, 3],
                TensorData::F32(logits),
            )])
        }

        async fn encode(
            &self,
            _ctx: &SecurityContext,
            _model_id: &str,
            text: &str,
        ) -> Result<TokenEncoding, OnnxRunnerError> {
            let mut ids = Vec::new();
            let mut offsets = Vec::new();
            let mut cursor = 0u32;
            for word in text.split_whitespace() {
                #[allow(clippy::cast_possible_truncation)] // test fixture, short strings only
                let start = text[cursor as usize..]
                    .find(word)
                    .map_or(cursor, |i| cursor + i as u32);
                #[allow(clippy::cast_possible_truncation)]
                let end = start + word.len() as u32;
                ids.push(1);
                offsets.push((start, end));
                cursor = end;
            }
            let len = ids.len();
            Ok(TokenEncoding {
                ids,
                attention_mask: vec![1; len],
                offsets,
                special_tokens_mask: vec![false; len],
            })
        }

        async fn encode_fragment(
            &self,
            _ctx: &SecurityContext,
            _model_id: &str,
            _word: &str,
        ) -> Result<Vec<i64>, OnnxRunnerError> {
            unimplemented!("not used by the BIO token classifier backend")
        }

        /// Declares the two inputs a `token_type_ids`-free export has, so the
        /// backend builds exactly those. A model that declared three would get
        /// three; that variability is the point of asking.
        async fn list_models(
            &self,
            _ctx: &SecurityContext,
        ) -> Result<Vec<ModelInfo>, OnnxRunnerError> {
            Ok(vec![ModelInfo {
                model_id: "minilm-l6-pii".to_owned(),
                state: onnx_runner_sdk::ModelState::Ready,
                loaded: Some(onnx_runner_sdk::LoadedModelInfo {
                    input_names: vec!["input_ids".to_owned(), "attention_mask".to_owned()],
                    output_names: vec!["logits".to_owned()],
                    sha256: String::new(),
                    replicas: 1,
                    labels: None,
                }),
            }])
        }
    }

    fn backend() -> BioTokenClassifierBackend {
        BioTokenClassifierBackend::new(
            Arc::new(FakeOnnxRunner),
            "minilm-l6-pii".to_owned(),
            labels(),
        )
    }

    #[tokio::test]
    async fn detects_entity_from_first_token() {
        let entities = backend()
            .detect_entities(&ctx(), "contact jane at example.com now")
            .await
            .unwrap();

        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].category, "EMAIL");
    }
}
