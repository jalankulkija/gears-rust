//! Span decoding, ported from `dlp-bench`'s
//! `src/detectors/gliner2/decode.rs` (`_extract_entities` / `_find_spans` /
//! `finalize_spans` in the `GLiNER2` reference).
//!
//! Four details that must be exact, each of which shifts F1 by points if
//! got wrong and none of which is guessable from the tensor shapes:
//!
//! 1. **`count_logits` is a hard gate.** `argmax == 0` means "no entities"
//!    and the document yields nothing, whatever the span probabilities
//!    say. It is evaluated once globally, not per label.
//! 2. **Cell `(start, width)` covers words `[start, start + width + 1)`** —
//!    inclusive end in the model's coordinates. Invalid cells are *not*
//!    masked inside the graph, so the bounds check is the only guard
//!    against reading a score that belongs to span `(0, 0)`.
//! 3. **Threshold is `>=`, not `>`.**
//! 4. **Suppression is per label.** Sort by descending probability, then
//!    ascending start and end; greedily keep, dropping anything
//!    overlapping an already-kept span *of the same label*. Cross-label
//!    overlaps survive, which is why a `PASSWORD` inside a
//!    `CONNECTION_STRING` is reportable.

use super::tensor::TensorOut;
use super::words::Word;

/// A decoded span in the model's own vocabulary.
#[derive(Debug, Clone, PartialEq)]
pub struct RawSpan {
    /// Index into the requested label list.
    pub label_idx: usize,
    /// UTF-8 byte offsets into the *prepared* text.
    pub start: usize,
    pub end: usize,
    pub score: f32,
}

/// Shape of the `span_probs` tensor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpanDims {
    pub num_words: usize,
    pub max_width: usize,
    pub num_labels: usize,
}

impl SpanDims {
    pub fn from_tensor(t: &TensorOut) -> Option<Self> {
        match t.shape.as_slice() {
            &[num_words, max_width, num_labels] => Some(Self {
                num_words,
                max_width,
                num_labels,
            }),
            _ => None,
        }
    }

    fn index(self, start: usize, width: usize, label: usize) -> usize {
        (start * self.max_width + width) * self.num_labels + label
    }
}

/// Whether the count head permits any entity output at all.
///
/// The count head was trained on structures, not entities, so beyond this
/// gate it contributes nothing to entity decoding — but the gate itself is
/// load bearing and skipping it changes results on documents the model
/// considers empty.
pub fn count_gate_open(count_logits: &TensorOut) -> bool {
    // Shape is [batch, 20]; batch is 1. argmax == 0 means "no entities".
    count_logits.argmax() != 0
}

/// Decode all labels.
pub fn decode(
    span_probs: &TensorOut,
    dims: SpanDims,
    count_logits: &TensorOut,
    words: &[Word],
    text: &str,
    threshold: f32,
) -> Vec<RawSpan> {
    if !count_gate_open(count_logits) {
        return Vec::new();
    }

    let num_words = dims.num_words.min(words.len());
    let mut out = Vec::new();
    let mut kept: Vec<RawSpan> = Vec::new();

    for label in 0..dims.num_labels {
        kept.clear();
        let mut candidates = find_spans(span_probs, dims, num_words, label, words, text, threshold);

        // Descending score, then ascending start and end. A total order
        // over distinct cells, so `sort_unstable_by` is deterministic here.
        candidates.sort_unstable_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then(a.start.cmp(&b.start))
                .then(a.end.cmp(&b.end))
        });

        for c in candidates {
            let overlaps = kept.iter().any(|k| !(c.end <= k.start || c.start >= k.end));
            if !overlaps {
                kept.push(c);
            }
        }
        out.append(&mut kept);
    }

    out
}

/// Collect above-threshold cells for one label.
fn find_spans(
    span_probs: &TensorOut,
    dims: SpanDims,
    num_words: usize,
    label: usize,
    words: &[Word],
    text: &str,
    threshold: f32,
) -> Vec<RawSpan> {
    let mut v = Vec::new();
    // Row-major (start ascending, then width ascending), mirroring the
    // order `torch.where` yields, so any downstream tie-break sees the
    // same sequence.
    for start in 0..num_words {
        for width in 0..dims.max_width {
            let p = span_probs.data[dims.index(start, width, label)];
            if p < threshold {
                continue;
            }
            // Cells beyond the text length are not masked in the graph;
            // they hold the score of span (0, 0) and would otherwise be
            // emitted.
            let end_word = start + width + 1;
            if end_word > num_words {
                break;
            }
            let bs = words[start].start;
            let be = words[end_word - 1].end;
            // The surface is trimmed but the offsets are not, matching the
            // reference. Trimming to whitespace means the span is
            // discarded.
            let Some(slice) = text.get(bs..be) else {
                continue;
            };
            if slice.trim().is_empty() {
                continue;
            }
            v.push(RawSpan {
                label_idx: label,
                start: bs,
                end: be,
                score: p,
            });
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::super::words;
    use super::*;

    fn dims(num_words: usize, max_width: usize, num_labels: usize) -> SpanDims {
        SpanDims {
            num_words,
            max_width,
            num_labels,
        }
    }

    fn probs(d: SpanDims, cells: &[(usize, usize, usize, f32)]) -> TensorOut {
        let mut data = vec![0.0f32; d.num_words * d.max_width * d.num_labels];
        for &(s, w, l, p) in cells {
            data[d.index(s, w, l)] = p;
        }
        TensorOut {
            shape: vec![d.num_words, d.max_width, d.num_labels],
            data,
        }
    }

    fn gate(open: bool) -> TensorOut {
        let mut data = vec![0.0f32; 20];
        data[if open { 3 } else { 0 }] = 1.0;
        TensorOut {
            shape: vec![1, 20],
            data,
        }
    }

    fn prep(text: &str) -> (String, Vec<Word>) {
        let p = words::prepare(text);
        (p.text, p.words)
    }

    #[test]
    fn dims_are_read_from_the_tensor_shape() {
        let t = TensorOut {
            shape: vec![5, 8, 17],
            data: vec![0.0; 5 * 8 * 17],
        };
        assert_eq!(SpanDims::from_tensor(&t), Some(dims(5, 8, 17)));
        let bad = TensorOut {
            shape: vec![5, 8],
            data: vec![],
        };
        assert_eq!(SpanDims::from_tensor(&bad), None);
    }

    #[test]
    fn count_gate_blocks_everything_when_argmax_is_zero() {
        let (text, ws) = prep("John Smith");
        let d = dims(ws.len(), 8, 1);
        let p = probs(d, &[(0, 1, 0, 0.99)]);
        assert!(decode(&p, d, &gate(false), &ws, &text, 0.5).is_empty());
        assert!(!decode(&p, d, &gate(true), &ws, &text, 0.5).is_empty());
    }

    #[test]
    fn threshold_is_inclusive() {
        let (text, ws) = prep("John Smith");
        let d = dims(ws.len(), 8, 1);
        let p = probs(d, &[(0, 1, 0, 0.5)]);
        assert_eq!(decode(&p, d, &gate(true), &ws, &text, 0.5).len(), 1);
        assert!(decode(&p, d, &gate(true), &ws, &text, 0.51).is_empty());
    }

    #[test]
    fn same_label_overlap_is_suppressed_keeping_the_higher_score() {
        let (text, ws) = prep("John Smith Jones");
        let d = dims(ws.len(), 8, 1);
        // (0, width 1) = "John Smith"; (0, width 2) = "John Smith Jones" — overlap.
        let p = probs(d, &[(0, 1, 0, 0.6), (0, 2, 0, 0.9)]);
        let out = decode(&p, d, &gate(true), &ws, &text, 0.5);
        assert_eq!(out.len(), 1);
        assert!(
            (out[0].score - 0.9).abs() < f32::EPSILON,
            "expected 0.9, got {}",
            out[0].score
        );
    }

    #[test]
    fn cross_label_overlap_survives() {
        let (text, ws) = prep("John Smith");
        let d = dims(ws.len(), 8, 2);
        let p = probs(d, &[(0, 1, 0, 0.9), (0, 1, 1, 0.8)]);
        let out = decode(&p, d, &gate(true), &ws, &text, 0.5);
        assert_eq!(out.len(), 2);
    }
}
