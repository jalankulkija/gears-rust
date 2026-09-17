//! BIO-tag decoding for a plain token-classification model (`minilm-l6-pii`
//! and similar) — see `gears/onnx-runner/docs/DESIGN.md`'s "Which PII
//! model" decision. No prompt encoding, no span-probability decoding
//! (that's `domain::gliner2`) — just per-token argmax over a fixed label
//! set, then merging consecutive `B-X`/`I-X` tokens into entity spans.

use crate::domain::entity::Entity;

/// A single token's predicted label and the model's confidence in it.
pub struct TokenPrediction {
    pub label: String,
    pub confidence: f32,
}

fn softmax(row: &[f32]) -> Vec<f32> {
    let max = row.iter().copied().fold(f32::MIN, f32::max);
    let exps: Vec<f32> = row.iter().map(|&v| (v - max).exp()).collect();
    let sum: f32 = exps.iter().sum();
    if sum <= 0.0 {
        return vec![0.0; row.len()];
    }
    exps.iter().map(|&e| e / sum).collect()
}

/// Per-token argmax over `logits` (flattened `[seq_len, labels.len()]`,
/// row-major), mapped to `labels`.
///
/// # Panics
/// Never — malformed shapes (fewer logits than `seq_len * labels.len()`)
/// simply yield fewer predictions than `seq_len`; callers should treat a
/// short result as a contract mismatch (see `domain::error`).
#[must_use]
pub fn argmax_predictions(
    logits: &[f32],
    seq_len: usize,
    labels: &[String],
) -> Vec<TokenPrediction> {
    let num_labels = labels.len();
    if num_labels == 0 {
        return Vec::new();
    }
    (0..seq_len)
        .filter_map(|i| {
            let row = logits.get(i * num_labels..(i + 1) * num_labels)?;
            let probs = softmax(row);
            let (best_idx, &best_prob) =
                probs.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?;
            Some(TokenPrediction {
                label: labels[best_idx].clone(),
                confidence: best_prob,
            })
        })
        .collect()
}

/// Splits a BIO label into its prefix (`'B'`, `'I'`, or `'O'`) and category
/// (empty for `'O'`).
fn split_bio(label: &str) -> (char, &str) {
    if let Some(rest) = label.strip_prefix("B-") {
        ('B', rest)
    } else if let Some(rest) = label.strip_prefix("I-") {
        ('I', rest)
    } else {
        ('O', "")
    }
}

struct OpenEntity {
    category: String,
    start: u32,
    end: u32,
    confidences: Vec<f32>,
}

impl OpenEntity {
    fn finish(self) -> Entity {
        #[allow(clippy::cast_precision_loss)] // confidence lists are always short
        let mean = self.confidences.iter().sum::<f32>() / self.confidences.len() as f32;
        Entity {
            category: self.category,
            start: self.start,
            end: self.end,
            confidence: mean,
        }
    }
}

/// Merges per-token BIO predictions into entity spans, using `offsets`
/// (byte spans into the original text, per token) and
/// `special_tokens_mask` (`[CLS]`/`[SEP]`/padding tokens, which always
/// close any open entity and are otherwise ignored).
///
/// A lenient merge: an `I-X` immediately after an unrelated `B-Y`/`I-Y` (or
/// with no preceding `B-`/`I-` at all) starts a new entity rather than being
/// dropped — real model output sometimes skips a `B-` tag on the first
/// token of a span.
#[must_use]
pub fn merge_bio(
    predictions: &[TokenPrediction],
    offsets: &[(u32, u32)],
    special_tokens_mask: &[bool],
) -> Vec<Entity> {
    let mut entities = Vec::new();
    let mut current: Option<OpenEntity> = None;

    for (i, pred) in predictions.iter().enumerate() {
        if special_tokens_mask.get(i).copied().unwrap_or(false) {
            if let Some(open) = current.take() {
                entities.push(open.finish());
            }
            continue;
        }
        let Some(&(start, end)) = offsets.get(i) else {
            continue;
        };
        let (prefix, category) = split_bio(&pred.label);

        match prefix {
            'O' => {
                if let Some(open) = current.take() {
                    entities.push(open.finish());
                }
            }
            'B' => {
                if let Some(open) = current.take() {
                    entities.push(open.finish());
                }
                current = Some(OpenEntity {
                    category: category.to_owned(),
                    start,
                    end,
                    confidences: vec![pred.confidence],
                });
            }
            'I' => match &mut current {
                Some(open) if open.category == category => {
                    open.end = end;
                    open.confidences.push(pred.confidence);
                }
                _ => {
                    if let Some(open) = current.take() {
                        entities.push(open.finish());
                    }
                    current = Some(OpenEntity {
                        category: category.to_owned(),
                        start,
                        end,
                        confidences: vec![pred.confidence],
                    });
                }
            },
            _ => unreachable!("split_bio only returns 'B', 'I', or 'O'"),
        }
    }

    if let Some(open) = current.take() {
        entities.push(open.finish());
    }
    entities
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels() -> Vec<String> {
        ["O", "B-EMAIL", "I-EMAIL", "B-PERSON", "I-PERSON"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    }

    #[test]
    fn argmax_predictions_picks_highest_logit() {
        let labels = labels();
        // One token, logits favor B-EMAIL (index 1).
        let logits = [0.1, 5.0, 0.2, 0.1, 0.1];
        let preds = argmax_predictions(&logits, 1, &labels);
        assert_eq!(preds.len(), 1);
        assert_eq!(preds[0].label, "B-EMAIL");
        assert!(preds[0].confidence > 0.9);
    }

    #[test]
    fn argmax_predictions_returns_empty_for_empty_labels() {
        assert!(argmax_predictions(&[1.0, 2.0], 1, &[]).is_empty());
    }

    fn pred(label: &str, confidence: f32) -> TokenPrediction {
        TokenPrediction {
            label: label.to_owned(),
            confidence,
        }
    }

    #[test]
    fn merge_bio_merges_consecutive_i_tags_into_one_entity() {
        let preds = vec![
            pred("O", 0.9),
            pred("B-EMAIL", 0.8),
            pred("I-EMAIL", 0.85),
            pred("O", 0.9),
        ];
        let offsets = vec![(0, 5), (6, 12), (12, 24), (25, 26)];
        let special = vec![false, false, false, false];

        let entities = merge_bio(&preds, &offsets, &special);
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].category, "EMAIL");
        assert_eq!(entities[0].start, 6);
        assert_eq!(entities[0].end, 24);
    }

    #[test]
    fn merge_bio_splits_entities_of_different_categories() {
        let preds = vec![pred("B-EMAIL", 0.9), pred("B-PERSON", 0.9)];
        let offsets = vec![(0, 5), (6, 10)];
        let special = vec![false, false];

        let entities = merge_bio(&preds, &offsets, &special);
        assert_eq!(entities.len(), 2);
        assert_eq!(entities[0].category, "EMAIL");
        assert_eq!(entities[1].category, "PERSON");
    }

    #[test]
    fn merge_bio_closes_entity_at_special_token() {
        let preds = vec![pred("B-EMAIL", 0.9), pred("O", 0.5)];
        let offsets = vec![(0, 5), (0, 0)];
        let special = vec![false, true];

        let entities = merge_bio(&preds, &offsets, &special);
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].end, 5);
    }

    #[test]
    fn merge_bio_treats_lone_i_tag_as_a_new_entity() {
        let preds = vec![pred("I-EMAIL", 0.7)];
        let offsets = vec![(0, 5)];
        let special = vec![false];

        let entities = merge_bio(&preds, &offsets, &special);
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].category, "EMAIL");
    }
}
