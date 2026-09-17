//! Compiled pattern set — the whole of this backend's detection.
//!
//! [`PatternSet::compile`] turns [`PatternConfig`] entries into regexes once,
//! at gear init, so an invalid pattern is a startup failure rather than a
//! per-request one. [`PatternSet::scan`] then runs them over a payload.
//!
//! # One pass, not one pass per pattern
//!
//! The obvious implementation runs each regex over the payload in turn: N
//! patterns means N scans of the text. Instead the compiled set keeps a
//! [`RegexSet`] alongside the individual regexes. `RegexSet` compiles every
//! pattern into a single automaton, so `matches()` answers "which of these
//! seventeen patterns occur?" in **one** pass, and only the patterns it names
//! are then re-run to extract spans.
//!
//! The common case for a scanning proxy is a prompt containing no secrets at
//! all, and that case collapses from seventeen passes to one. A payload with a
//! single email costs one pass plus one. `RegexSet` cannot report capture
//! groups or offsets — hence the second pass — but it does not need to.

use std::collections::HashMap;

use prompt_scanner_sdk::VerdictAction;
use regex::{Regex, RegexSet};

use crate::config::{PatternConfig, Tier, Validator};
use crate::domain::validators;

/// A single match against the scanned payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub category: String,
    /// Byte offsets of capture group 0, always — even when a validator
    /// inspected a different group.
    pub span: (u32, u32),
    pub tier: Tier,
    pub suggested_action: VerdictAction,
}

/// Why a pattern set could not be compiled. Surfaced from `Gear::init`, so the
/// message names the offending pattern rather than just the regex error.
#[derive(Debug, thiserror::Error)]
pub enum CompileError {
    #[error("pattern {index} (category `{category}`) has an invalid regex: {source}")]
    InvalidRegex {
        index: usize,
        category: String,
        #[source]
        source: Box<regex::Error>,
    },
    #[error(
        "pattern {index} (category `{category}`) names capture group {capture}, but its regex has \
         only {available} group(s)"
    )]
    MissingCaptureGroup {
        index: usize,
        category: String,
        capture: usize,
        available: usize,
    },
    #[error("pattern {index} (category `{category}`) has an empty category")]
    EmptyCategory { index: usize, category: String },
}

#[derive(Debug)]
struct Compiled {
    category: String,
    regex: Regex,
    tier: Tier,
    action: VerdictAction,
    validator: Option<Validator>,
    capture: usize,
}

/// Compiled, ready-to-run detection patterns.
#[derive(Debug)]
pub struct PatternSet {
    /// Single-automaton prefilter over every pattern; indices line up with
    /// `patterns`.
    set: RegexSet,
    patterns: Vec<Compiled>,
    entropy_threshold: f64,
}

impl PatternSet {
    /// Compile `patterns`. Returns the first error rather than skipping bad
    /// entries: a silently dropped pattern is a detector that stops detecting
    /// without telling anyone.
    ///
    /// # Errors
    ///
    /// [`CompileError`] when a regex does not parse, a pattern names a capture
    /// group its regex does not have, or a category is empty.
    pub fn compile(
        patterns: &[PatternConfig],
        entropy_threshold: f64,
    ) -> Result<Self, CompileError> {
        let mut compiled = Vec::with_capacity(patterns.len());

        for (index, cfg) in patterns.iter().enumerate() {
            if cfg.category.trim().is_empty() {
                return Err(CompileError::EmptyCategory {
                    index,
                    category: cfg.category.clone(),
                });
            }

            let regex = Regex::new(&cfg.regex).map_err(|source| CompileError::InvalidRegex {
                index,
                category: cfg.category.clone(),
                source: Box::new(source),
            })?;

            // `captures_len` counts group 0, so group N needs captures_len > N.
            let available = regex.captures_len().saturating_sub(1);
            if cfg.capture > available {
                return Err(CompileError::MissingCaptureGroup {
                    index,
                    category: cfg.category.clone(),
                    capture: cfg.capture,
                    available,
                });
            }

            compiled.push(Compiled {
                category: cfg.category.clone(),
                regex,
                tier: cfg.tier,
                action: cfg.action,
                validator: cfg.validator,
                capture: cfg.capture,
            });
        }

        // Every regex here already parsed individually, so the set cannot fail
        // to compile for a syntax reason; it can still exceed the size limit,
        // which is a real configuration error worth reporting as one.
        let set = RegexSet::new(patterns.iter().map(|c| &c.regex)).map_err(|source| {
            CompileError::InvalidRegex {
                index: 0,
                category: "<pattern set>".to_owned(),
                source: Box::new(source),
            }
        })?;

        Ok(Self {
            set,
            patterns: compiled,
            entropy_threshold,
        })
    }

    /// Number of compiled patterns.
    #[must_use]
    pub fn len(&self) -> usize {
        self.patterns.len()
    }

    /// Whether the set detects nothing — a valid, explicitly configured state.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Every match every pattern finds, each reported separately. Overlaps are
    /// kept: one payload can hold several distinct pieces of sensitive
    /// material, and the gateway merges spans only at redaction time.
    #[must_use]
    pub fn scan(&self, text: &str) -> Vec<Match> {
        if self.patterns.is_empty() {
            return Vec::new();
        }

        let mut out = Vec::new();
        // One pass over `text` for all patterns; only the hits are re-run.
        for index in self.set.matches(text) {
            let Some(pattern) = self.patterns.get(index) else {
                continue;
            };
            self.collect_pattern_matches(pattern, text, &mut out);
        }
        out
    }

    fn collect_pattern_matches(&self, pattern: &Compiled, text: &str, out: &mut Vec<Match>) {
        // `find_iter` skips capture bookkeeping, so use it whenever the
        // validator inspects group 0 (or there is no validator at all).
        if pattern.capture == 0 {
            for m in pattern.regex.find_iter(text) {
                if self.accepts(pattern, m.as_str()) {
                    out.push(Self::to_match(pattern, m.start(), m.end()));
                }
            }
            return;
        }

        for caps in pattern.regex.captures_iter(text) {
            let (Some(whole), Some(subject)) = (caps.get(0), caps.get(pattern.capture)) else {
                continue;
            };
            if self.accepts(pattern, subject.as_str()) {
                out.push(Self::to_match(pattern, whole.start(), whole.end()));
            }
        }
    }

    fn to_match(pattern: &Compiled, start: usize, end: usize) -> Match {
        Match {
            category: pattern.category.clone(),
            span: span_u32(start, end),
            tier: pattern.tier,
            suggested_action: pattern.action,
        }
    }

    fn accepts(&self, pattern: &Compiled, candidate: &str) -> bool {
        let Some(validator) = pattern.validator else {
            return true;
        };
        match validator {
            Validator::Luhn => {
                let digits: String = candidate.chars().filter(char::is_ascii_digit).collect();
                (13..=19).contains(&digits.len()) && validators::luhn_check(&digits)
            }
            Validator::Iban => {
                let normalized: String = candidate.chars().filter(|c| !c.is_whitespace()).collect();
                validators::iban_checksum_valid(&normalized)
            }
            Validator::Ipv4 => validators::is_valid_ipv4(candidate),
            Validator::Ipv6 => validators::is_valid_ipv6(candidate),
            Validator::Jwt => validators::looks_like_jwt(candidate),
            Validator::Entropy => validators::shannon_entropy(candidate) >= self.entropy_threshold,
        }
    }

    /// Count of patterns per category, for the startup log.
    #[must_use]
    pub fn categories(&self) -> HashMap<&str, usize> {
        let mut counts = HashMap::new();
        for pattern in &self.patterns {
            *counts.entry(pattern.category.as_str()).or_insert(0) += 1;
        }
        counts
    }
}

/// Byte offsets to the `(u32, u32)` span carried on a `Finding` — payloads
/// scanned here never approach `u32::MAX` bytes.
#[allow(clippy::cast_possible_truncation)]
const fn span_u32(start: usize, end: usize) -> (u32, u32) {
    (start as u32, end as u32)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::config::default_patterns;

    fn builtin() -> PatternSet {
        PatternSet::compile(&default_patterns(), 3.0).expect("built-in patterns must compile")
    }

    fn categories(text: &str) -> Vec<String> {
        let mut c: Vec<String> = builtin()
            .scan(text)
            .into_iter()
            .map(|m| m.category)
            .collect();
        c.sort();
        c.dedup();
        c
    }

    // ── the built-in set still detects what the recognizers detected ────────

    #[test]
    fn detects_email() {
        assert!(
            categories("contact me at leaked@example.com please").contains(&"pii.email".to_owned())
        );
    }

    #[test]
    fn detects_url() {
        assert!(
            categories("see https://example.com/path?q=1 for details")
                .contains(&"pii.url".to_owned())
        );
    }

    #[test]
    fn validated_ipv4_is_detected_and_garbage_is_not() {
        assert!(
            categories("server at 192.168.1.1 responded").contains(&"pii.ip_address".to_owned())
        );
        assert!(
            !categories("version 999.999.999.999 is invalid")
                .contains(&"pii.ip_address".to_owned())
        );
    }

    #[test]
    fn detects_phone_number() {
        assert!(categories("call me at +1 555-123-4567 today").contains(&"pii.phone".to_owned()));
    }

    #[test]
    fn luhn_valid_card_is_detected_and_invalid_is_not() {
        assert!(
            categories("card 4111111111111111 on file")
                .contains(&"financial.credit_card".to_owned())
        );
        assert!(
            !categories("card 4111111111111112 on file")
                .contains(&"financial.credit_card".to_owned())
        );
    }

    #[test]
    fn detects_vendor_key_and_private_key() {
        assert!(categories("key is AKIAABCDEFGHIJKLMNOP").contains(&"secret.aws_key".to_owned()));
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIBOgIBAAJBAK\n-----END RSA PRIVATE KEY-----";
        assert!(categories(pem).contains(&"secret.private_key".to_owned()));
    }

    #[test]
    fn clean_text_yields_nothing() {
        assert!(builtin().scan("nothing sensitive here").is_empty());
    }

    #[test]
    fn several_distinct_findings_from_one_payload() {
        let found = categories("mail leaked@example.com and key AKIAABCDEFGHIJKLMNOP");
        assert!(found.contains(&"pii.email".to_owned()));
        assert!(found.contains(&"secret.aws_key".to_owned()));
    }

    // ── tier, span and validator plumbing ───────────────────────────────────

    #[test]
    fn tier_determines_reported_confidence() {
        assert!((Tier::Pattern.confidence() - 0.5).abs() < f32::EPSILON);
        assert!((Tier::Structural.confidence() - 0.75).abs() < f32::EPSILON);
        assert!((Tier::Validated.confidence() - 0.95).abs() < f32::EPSILON);

        let m = builtin()
            .scan("server at 192.168.1.1 responded")
            .into_iter()
            .find(|m| m.category == "pii.ip_address")
            .expect("ipv4 match");
        assert_eq!(
            m.tier,
            Tier::Validated,
            "a checksum-validated match is Validated"
        );
    }

    #[test]
    fn span_covers_group_zero_even_when_the_validator_reads_another_group() {
        // `secret.generic` validates capture 1 (the value) but must report the
        // whole `key=value` match.
        let text = "api_key = s3cr3tV4lueWithEntropy99";
        let m = builtin()
            .scan(text)
            .into_iter()
            .find(|m| m.category == "secret.generic")
            .expect("generic secret match");
        let (start, end) = m.span;
        let matched = &text[start as usize..end as usize];
        assert!(
            matched.starts_with("api_key"),
            "span must cover group 0, got {matched:?}"
        );
    }

    #[test]
    fn entropy_threshold_is_honoured() {
        let low = "password = changeme";
        let patterns = default_patterns();

        let permissive = PatternSet::compile(&patterns, 0.0).expect("compile");
        assert!(
            permissive
                .scan(low)
                .iter()
                .any(|m| m.category == "secret.generic"),
            "a zero threshold accepts a low-entropy value"
        );

        let strict = PatternSet::compile(&patterns, 6.0).expect("compile");
        assert!(
            !strict
                .scan(low)
                .iter()
                .any(|m| m.category == "secret.generic"),
            "a high threshold rejects it"
        );
    }

    // ── compilation is a startup gate ───────────────────────────────────────

    #[test]
    fn an_invalid_regex_fails_compilation_and_names_the_pattern() {
        let bad = vec![PatternConfig {
            category: "broken.thing".to_owned(),
            regex: "((unclosed".to_owned(),
            tier: Tier::Pattern,
            action: VerdictAction::Warn,
            validator: None,
            capture: 0,
        }];
        let err = PatternSet::compile(&bad, 3.0).expect_err("must not compile");
        let msg = err.to_string();
        assert!(
            msg.contains("broken.thing"),
            "error names the pattern: {msg}"
        );
    }

    #[test]
    fn a_capture_group_the_regex_lacks_fails_compilation() {
        let bad = vec![PatternConfig {
            category: "broken.capture".to_owned(),
            regex: r"\bAKIA[0-9A-Z]{16}\b".to_owned(),
            tier: Tier::Pattern,
            action: VerdictAction::Warn,
            validator: Some(Validator::Entropy),
            capture: 2,
        }];
        let err = PatternSet::compile(&bad, 3.0).expect_err("must not compile");
        assert!(
            matches!(err, CompileError::MissingCaptureGroup { capture: 2, .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn an_empty_category_fails_compilation() {
        let bad = vec![PatternConfig {
            category: "  ".to_owned(),
            regex: "x".to_owned(),
            tier: Tier::Pattern,
            action: VerdictAction::Warn,
            validator: None,
            capture: 0,
        }];
        assert!(matches!(
            PatternSet::compile(&bad, 3.0),
            Err(CompileError::EmptyCategory { .. })
        ));
    }

    #[test]
    fn an_explicitly_empty_pattern_set_is_valid_and_detects_nothing() {
        let empty = PatternSet::compile(&[], 3.0).expect("an empty set is valid");
        assert!(empty.is_empty());
        assert!(
            empty
                .scan("card 4111111111111111 leaked@example.com")
                .is_empty()
        );
    }

    #[test]
    fn a_custom_pattern_set_replaces_the_built_ins() {
        let custom = vec![PatternConfig {
            category: "custom.ticket".to_owned(),
            regex: r"\bTICKET-\d{4}\b".to_owned(),
            tier: Tier::Structural,
            action: VerdictAction::Audit,
            validator: None,
            capture: 0,
        }];
        let set = PatternSet::compile(&custom, 3.0).expect("compile");
        let found = set.scan("see TICKET-1234 and mail a@b.com");

        assert_eq!(found.len(), 1, "only the configured pattern runs");
        assert_eq!(found[0].category, "custom.ticket");
        assert_eq!(found[0].tier, Tier::Structural);
        assert_eq!(found[0].suggested_action, VerdictAction::Audit);
    }
}
