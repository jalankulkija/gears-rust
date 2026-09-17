//! Corpus loading.
//!
//! One JSON object per line. A labelled document carries gold spans; a
//! negative control carries none *and says so*, because "no spans" and "not
//! annotated" score identically and mean opposite things:
//!
//! ```json
//! {"id":"a1","lang":"en","text":"Email sarah@example.com","spans":[{"category":"EMAIL","start":6,"end":23}]}
//! {"id":"n1","lang":"en","text":"It was a bright cold day in April","negative_control":true}
//! ```
//!
//! JSONL rather than one big array so a partial file is still usable and a
//! malformed record names its own line number.

use std::collections::BTreeSet;
use std::path::Path;

use serde::Deserialize;

use super::scoring::{Document, GoldSpan};

#[derive(Debug, Deserialize)]
struct RawSpan {
    category: String,
    start: u32,
    end: u32,
}

#[derive(Debug, Deserialize)]
struct RawDocument {
    id: String,
    text: String,
    #[serde(default)]
    lang: Option<String>,
    #[serde(default)]
    spans: Vec<RawSpan>,
    /// Asserts the text contains no PII at all. Required for a document with
    /// no spans; see [`CorpusError::UnlabelledDocument`].
    #[serde(default)]
    negative_control: bool,
}

/// A loaded corpus, with the two kinds kept apart because they are scored
/// differently and must never be pooled.
#[derive(Debug, Default)]
pub struct Corpus {
    pub labelled: Vec<Document>,
    pub negative_controls: Vec<Document>,
}

impl Corpus {
    /// Every gold category present, for checking a taxonomy map covers the
    /// corpus before a long run rather than after it.
    #[must_use]
    pub fn gold_categories(&self) -> BTreeSet<String> {
        self.labelled
            .iter()
            .flat_map(|d| d.spans.iter().map(|s| s.category.clone()))
            .collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CorpusError {
    #[error("reading {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}:{line}: {source}")]
    Malformed {
        path: String,
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    /// A document with no spans that does not claim to be a negative control.
    ///
    /// Rejected rather than assumed either way: treated as labelled it
    /// contributes phantom false positives, and treated as a negative control
    /// it silently asserts something about text nobody checked. Both corrupt
    /// the result quietly, which is the failure mode worth refusing.
    #[error(
        "{path}:{line}: document '{id}' has no spans and is not marked \
         `negative_control`. Add the spans, or set the flag to assert the text \
         is PII-free."
    )]
    UnlabelledDocument {
        path: String,
        line: usize,
        id: String,
    },
    #[error(
        "{path}:{line}: document '{id}' is marked `negative_control` but carries {count} spans"
    )]
    ContradictoryDocument {
        path: String,
        line: usize,
        id: String,
        count: usize,
    },
    #[error(
        "{path}:{line}: document '{id}' span {start}..{end} is not a character boundary of its text"
    )]
    SpanNotOnBoundary {
        path: String,
        line: usize,
        id: String,
        start: u32,
        end: u32,
    },
}

/// Reads a JSONL corpus, validating as it goes.
///
/// # Errors
///
/// Any malformed, contradictory or unlabelled record, and any span whose
/// offsets do not land on UTF-8 character boundaries of its own text — the
/// last of which is worth catching here, because a benchmark that silently
/// compares against unusable offsets produces plausible numbers.
pub fn load(path: &Path) -> Result<Corpus, CorpusError> {
    let display = path.display().to_string();
    let raw = std::fs::read_to_string(path).map_err(|source| CorpusError::Io {
        path: display.clone(),
        source,
    })?;

    let mut corpus = Corpus::default();

    for (index, line) in raw.lines().enumerate() {
        let line_no = index + 1;
        if line.trim().is_empty() {
            continue;
        }

        let record: RawDocument =
            serde_json::from_str(line).map_err(|source| CorpusError::Malformed {
                path: display.clone(),
                line: line_no,
                source,
            })?;

        if record.negative_control && !record.spans.is_empty() {
            return Err(CorpusError::ContradictoryDocument {
                path: display,
                line: line_no,
                id: record.id,
                count: record.spans.len(),
            });
        }
        if !record.negative_control && record.spans.is_empty() {
            return Err(CorpusError::UnlabelledDocument {
                path: display,
                line: line_no,
                id: record.id,
            });
        }

        for span in &record.spans {
            let (start, end) = (span.start as usize, span.end as usize);
            let usable = start <= end
                && end <= record.text.len()
                && record.text.is_char_boundary(start)
                && record.text.is_char_boundary(end);
            if !usable {
                return Err(CorpusError::SpanNotOnBoundary {
                    path: display,
                    line: line_no,
                    id: record.id.clone(),
                    start: span.start,
                    end: span.end,
                });
            }
        }

        let document = Document {
            id: record.id,
            text: record.text,
            lang: record.lang,
            spans: record
                .spans
                .into_iter()
                .map(|s| GoldSpan {
                    category: s.category,
                    start: s.start,
                    end: s.end,
                })
                .collect(),
        };

        if record.negative_control {
            corpus.negative_controls.push(document);
        } else {
            corpus.labelled.push(document);
        }
    }

    Ok(corpus)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(contents: &str) -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        file.write_all(contents.as_bytes()).expect("write");
        file
    }

    #[test]
    fn splits_labelled_from_negative_controls() {
        let file = write(
            r#"{"id":"a","lang":"en","text":"mail me","spans":[{"category":"EMAIL","start":0,"end":4}]}
{"id":"b","lang":"en","text":"nothing here","negative_control":true}
"#,
        );
        let corpus = load(file.path()).expect("loads");
        assert_eq!(corpus.labelled.len(), 1);
        assert_eq!(corpus.negative_controls.len(), 1);
        assert_eq!(corpus.gold_categories().len(), 1);
    }

    #[test]
    fn a_document_with_no_spans_must_declare_itself() {
        let file = write(r#"{"id":"x","text":"who knows"}"#);
        let err = load(file.path()).expect_err("must refuse");
        assert!(
            matches!(err, CorpusError::UnlabelledDocument { .. }),
            "got {err}"
        );
    }

    #[test]
    fn a_negative_control_with_spans_is_a_contradiction() {
        let file = write(
            r#"{"id":"x","text":"mail me","negative_control":true,"spans":[{"category":"EMAIL","start":0,"end":4}]}"#,
        );
        assert!(matches!(
            load(file.path()).expect_err("must refuse"),
            CorpusError::ContradictoryDocument { .. }
        ));
    }

    #[test]
    fn spans_must_land_on_character_boundaries() {
        // Byte 1 is inside a two-byte character. Comparing against this would
        // produce numbers rather than an error, which is the danger.
        // A Cyrillic word, built rather than written literally so the source
        // stays ASCII. Byte 1 falls inside its first two-byte character.
        let cyrillic = "\u{43f}\u{440}\u{438}\u{432}\u{435}\u{442}";
        let file = write(&format!(
            r#"{{"id":"x","text":"{cyrillic}","spans":[{{"category":"PERSON","start":1,"end":3}}]}}"#
        ));
        assert!(matches!(
            load(file.path()).expect_err("must refuse"),
            CorpusError::SpanNotOnBoundary { .. }
        ));
    }

    #[test]
    fn a_malformed_line_names_its_line_number() {
        let file = write("{\"id\":\"a\",\"text\":\"ok\",\"negative_control\":true}\nnot json\n");
        let err = load(file.path()).expect_err("must refuse");
        assert!(format!("{err}").contains(":2:"), "got {err}");
    }
}
