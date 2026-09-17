//! Span-based redaction: replaces only the detected `Redact`-suggesting
//! spans within a payload, instead of blanking the whole thing. Shared
//! between `prompt-scanner`'s own audit-trail redaction and connector
//! plugins (e.g. `openai-interceptor-plugin`) that need to modify a request
//! body in place, so both apply the exact same span-merging/fallback rules.

use crate::models::{Finding, VerdictAction};

/// Placeholder substituted for each redacted span (or for the whole text,
/// in the fallback case below).
pub const REDACTED_PLACEHOLDER: &str = "[REDACTED]";

/// Replaces every `Redact`-suggesting finding's span in `text` with
/// [`REDACTED_PLACEHOLDER`], leaving the rest of the text untouched.
/// Overlapping/adjacent spans are merged into one placeholder rather than
/// producing back-to-back duplicates.
///
/// Falls back to redacting the *entire* text (never leaves anything
/// unredacted) when:
/// - any `Redact`-suggesting finding has no span (can't be surgical about
///   something with no location), or
/// - a span is malformed (out of bounds, inverted, or not on a UTF-8 char
///   boundary — byte offsets should always land cleanly here since they
///   come from regex/tokenizer matches against this exact text, but this
///   is the failure-safe path if that ever isn't true).
///
/// Returns `text` unchanged if there are no `Redact`-suggesting findings at
/// all (callers are expected to only reach for this when the aggregated
/// verdict action is `Redact`, but this makes the function correct
/// regardless).
#[must_use]
pub fn redact_spans(text: &str, findings: &[Finding]) -> String {
    let redact_findings: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.suggested_action == VerdictAction::Redact)
        .collect();

    if redact_findings.is_empty() {
        return text.to_owned();
    }
    if redact_findings.iter().any(|f| f.span.is_none()) {
        return REDACTED_PLACEHOLDER.to_owned();
    }

    let mut spans: Vec<(usize, usize)> = redact_findings
        .iter()
        .filter_map(|f| f.span)
        .map(|s| (s.start as usize, s.end as usize))
        .filter(|&(start, end)| {
            start <= end
                && end <= text.len()
                && text.is_char_boundary(start)
                && text.is_char_boundary(end)
        })
        .collect();

    if spans.is_empty() {
        // Every span was malformed - fail safe rather than leak anything.
        return REDACTED_PLACEHOLDER.to_owned();
    }

    spans.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(spans.len());
    for (start, end) in spans {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }

    let mut result = String::with_capacity(text.len());
    let mut cursor = 0;
    for (start, end) in merged {
        result.push_str(&text[cursor..start]);
        result.push_str(REDACTED_PLACEHOLDER);
        cursor = end;
    }
    result.push_str(&text[cursor..]);
    result
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)] // test fixtures, short strings only
mod tests {
    use super::*;
    use crate::models::{Severity, Span};

    fn finding(category: &str, action: VerdictAction, span: Option<Span>) -> Finding {
        Finding {
            detector: "test".to_owned(),
            category: category.to_owned(),
            severity: Severity::Medium,
            confidence: 0.9,
            span,
            suggested_action: action,
        }
    }

    fn span(start: u32, end: u32) -> Span {
        Span { start, end }
    }

    #[test]
    fn redacts_only_the_matched_span() {
        let text = "Hi, my email is jane.doe@example.com, please get back to me.";
        let start = text
            .find("jane.doe@example.com")
            .expect("fixture contains the email") as u32;
        let end = start + "jane.doe@example.com".len() as u32;
        let findings = vec![finding(
            "pii.email",
            VerdictAction::Redact,
            Some(span(start, end)),
        )];

        let redacted = redact_spans(text, &findings);

        assert_eq!(
            redacted,
            "Hi, my email is [REDACTED], please get back to me."
        );
    }

    #[test]
    fn redacts_multiple_non_overlapping_spans() {
        let text = "email a@b.com and phone 555-1234 both here";
        let email_start = text.find("a@b.com").expect("fixture") as u32;
        let email_end = email_start + "a@b.com".len() as u32;
        let phone_start = text.find("555-1234").expect("fixture") as u32;
        let phone_end = phone_start + "555-1234".len() as u32;

        let findings = vec![
            finding(
                "pii.email",
                VerdictAction::Redact,
                Some(span(email_start, email_end)),
            ),
            finding(
                "pii.phone",
                VerdictAction::Redact,
                Some(span(phone_start, phone_end)),
            ),
        ];

        let redacted = redact_spans(text, &findings);

        assert_eq!(redacted, "email [REDACTED] and phone [REDACTED] both here");
    }

    #[test]
    fn merges_overlapping_spans_into_one_placeholder() {
        let text = "0123456789";
        let findings = vec![
            finding("a", VerdictAction::Redact, Some(span(2, 6))),
            finding("b", VerdictAction::Redact, Some(span(4, 8))),
        ];

        let redacted = redact_spans(text, &findings);

        assert_eq!(redacted, "01[REDACTED]89");
    }

    #[test]
    fn ignores_findings_that_do_not_suggest_redact() {
        let text = "just chatting, nothing sensitive";
        let findings = vec![finding("pii.email", VerdictAction::Warn, Some(span(0, 4)))];

        assert_eq!(redact_spans(text, &findings), text);
    }

    #[test]
    fn falls_back_to_whole_text_when_a_redact_finding_has_no_span() {
        let text = "some text";
        let findings = vec![finding("pii.generic", VerdictAction::Redact, None)];

        assert_eq!(redact_spans(text, &findings), REDACTED_PLACEHOLDER);
    }

    #[test]
    fn falls_back_to_whole_text_when_span_is_out_of_bounds() {
        let text = "short";
        let findings = vec![finding(
            "pii.email",
            VerdictAction::Redact,
            Some(span(0, 100)),
        )];

        assert_eq!(redact_spans(text, &findings), REDACTED_PLACEHOLDER);
    }

    #[test]
    fn no_redact_findings_leaves_text_unchanged() {
        let text = "hello world";
        assert_eq!(redact_spans(text, &[]), text);
    }
}
