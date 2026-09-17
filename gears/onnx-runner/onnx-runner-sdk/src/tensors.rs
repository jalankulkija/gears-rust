//! Input-building helpers.
//!
//! Convenience only. Nothing here is part of the gear's contract — these are
//! free functions over the same public types a consumer could assemble by
//! hand, and the gear neither knows nor cares whether they were used.
//!
//! They exist because tensor assembly is the most mechanical and most
//! error-prone step in calling [`crate::OnnxRunnerClientV1::run`]: a wrong
//! shape or a missing input is a runtime failure on live traffic, and every
//! consumer of a BERT-style model writes the same dozen lines to avoid it.

use crate::errors::OnnxRunnerError;
use crate::models::{NamedTensor, TensorData, TokenEncoding};

/// Builds the input tensors a BERT-style token classifier expects, from an
/// encoding and the graph's declared input names.
///
/// Supplies `input_ids` and `attention_mask` always, and `token_type_ids` —
/// zero-filled, single-segment — only when the graph declares it. That
/// conditional is the point: some exports declare it and some do not, omitting
/// a declared input is an error, and supplying an undeclared one is too, so the
/// correct set cannot be hardcoded.
///
/// Pass `declared_inputs` from [`crate::LoadedModelInfo::input_names`]; a name
/// this function cannot synthesize is an error rather than a silent omission,
/// because the failure would otherwise surface as a shape error from ONNX
/// Runtime with no indication of which input was missing.
///
/// # Batch size
///
/// Shapes are `[1, tokens]`. Coalescing several texts into one call is a
/// consumer decision with its own costs — the texts must be padded to a common
/// length and the results split apart again — so it is deliberately not done
/// here.
///
/// # Errors
///
/// [`OnnxRunnerError::ContractMismatch`] when `declared_inputs` names something
/// outside the set above.
pub fn token_classifier_inputs(
    model_id: &str,
    encoding: &TokenEncoding,
    declared_inputs: &[String],
) -> Result<Vec<NamedTensor>, OnnxRunnerError> {
    let n = encoding.ids.len();
    let mut inputs = Vec::with_capacity(declared_inputs.len());

    for name in declared_inputs {
        let data = match name.as_str() {
            "input_ids" => encoding.ids.clone(),
            "attention_mask" => encoding.attention_mask.clone(),
            // Single segment. The second-sentence case this distinguishes does
            // not arise when scanning one span of text.
            "token_type_ids" | "position_ids" => vec![0; n],
            other => {
                return Err(OnnxRunnerError::ContractMismatch {
                    model_id: model_id.to_owned(),
                    message: format!(
                        "input '{other}' is not a BERT-style token-classifier input; this \
                         helper builds input_ids, attention_mask, token_type_ids and \
                         position_ids only. Build this model's inputs directly."
                    ),
                });
            }
        };
        inputs.push(NamedTensor::new(
            name.clone(),
            vec![1, n],
            TensorData::I64(data),
        ));
    }

    Ok(inputs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoding(n: usize) -> TokenEncoding {
        TokenEncoding {
            ids: (0..i64::try_from(n).expect("test size fits")).collect(),
            attention_mask: vec![1; n],
            offsets: vec![(0, 1); n],
            special_tokens_mask: vec![false; n],
        }
    }

    #[test]
    fn builds_only_what_the_graph_declares() {
        let declared = vec!["input_ids".to_owned(), "attention_mask".to_owned()];
        let out = token_classifier_inputs("m", &encoding(4), &declared).expect("builds");

        assert_eq!(out.len(), 2, "token_type_ids must not appear undeclared");
        assert_eq!(out[0].name, "input_ids");
        assert_eq!(out[0].shape, vec![1, 4]);
    }

    #[test]
    fn adds_token_type_ids_when_declared() {
        let declared = vec![
            "input_ids".to_owned(),
            "attention_mask".to_owned(),
            "token_type_ids".to_owned(),
        ];
        let out = token_classifier_inputs("m", &encoding(3), &declared).expect("builds");

        assert_eq!(out.len(), 3);
        let tt = out
            .iter()
            .find(|t| t.name == "token_type_ids")
            .expect("present");
        assert_eq!(tt.data, TensorData::I64(vec![0, 0, 0]));
    }

    #[test]
    fn order_follows_the_declaration() {
        let declared = vec!["attention_mask".to_owned(), "input_ids".to_owned()];
        let out = token_classifier_inputs("m", &encoding(2), &declared).expect("builds");

        let names: Vec<_> = out.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["attention_mask", "input_ids"]);
    }

    #[test]
    fn unknown_input_is_an_error_not_an_omission() {
        // A GLiNER graph. Silently returning two tensors here would surface as
        // an opaque shape error from ONNX Runtime instead.
        let declared = vec!["input_ids".to_owned(), "span_idx".to_owned()];
        let err =
            token_classifier_inputs("gliner", &encoding(2), &declared).expect_err("must reject");

        assert!(
            format!("{err}").contains("span_idx"),
            "names the offender: {err}"
        );
    }
}
