//! Per-tenant detector policy helpers: threshold filtering, category action
//! overrides, and verdict aggregation.
//!
//! # Verdict-aggregation precedence rule
//!
//! `docs/DESIGN.md` leaves the exact aggregation rule as an open question
//! ("needs a precedence policy"). This gear picks the simplest sound rule
//! and documents it here rather than modeling per-tenant weighting schemes:
//!
//! 1. Findings below the tenant's configured confidence threshold for their
//!    category are dropped (a low-confidence "block" from a noisy detector
//!    should not out-rank nothing at all).
//! 2. Each remaining finding's `suggested_action` is replaced by the
//!    longest-prefix-matching entry in `TenantPolicy.category_action_overrides`,
//!    if any (see `apply_action_override`). Empty by default — detectors'
//!    own suggested actions are authoritative unless a policy says otherwise.
//! 3. Among the remaining findings, the aggregated action is the **maximum**
//!    `VerdictAction` by declaration order: `Block > Redact > Warn > Audit >
//!    Allow`. A single detector voting `Block` always wins, regardless of
//!    how many other detectors vote lower or how confident they are.
//! 4. If no findings remain after filtering, the tenant's
//!    `default_action` applies.
//!
//! This is intentionally simple; smarter aggregation (confidence-weighted
//! voting, per-category default actions) is left as future work.

use prompt_scanner_sdk::{Finding, TenantPolicy, VerdictAction};

/// Returns `true` if `finding` clears the tenant's configured confidence
/// threshold for its category. Categories with no configured threshold
/// always pass (threshold defaults to `0.0`).
#[must_use]
pub(crate) fn passes_threshold(finding: &Finding, policy: &TenantPolicy) -> bool {
    let threshold = policy
        .thresholds
        .get(&finding.category)
        .copied()
        .unwrap_or(0.0);
    finding.confidence >= threshold
}

/// Returns the longest key in `policy.category_action_overrides` that is a
/// prefix of `category`, if any — e.g. an override keyed `"pii"` applies to
/// `"pii.email"`, `"pii.url"`, etc. Longest-prefix-match mirrors how more
/// specific overrides (say, `"pii.email"` vs. `"pii"`) should win over a
/// broader one, without needing a separate specificity field.
fn matching_override(category: &str, policy: &TenantPolicy) -> Option<VerdictAction> {
    policy
        .category_action_overrides
        .iter()
        .filter(|(prefix, _)| category.starts_with(prefix.as_str()))
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(_, action)| *action)
}

/// Applies `policy.category_action_overrides` to `finding.suggested_action`
/// in place, if a matching override is configured.
fn apply_action_override(finding: &mut Finding, policy: &TenantPolicy) {
    if let Some(action) = matching_override(&finding.category, policy) {
        finding.suggested_action = action;
    }
}

/// Aggregate a set of detector findings into a single verdict action, per
/// the precedence rule documented above.
///
/// Returns `(action, findings_considered)` — `findings_considered` is the
/// subset of `findings` that passed the threshold filter (the findings
/// actually recorded against the resulting verdict), with any configured
/// `category_action_overrides` already applied to `suggested_action`.
#[must_use]
pub(crate) fn aggregate_verdict(
    findings: &[Finding],
    policy: &TenantPolicy,
) -> (VerdictAction, Vec<Finding>) {
    let considered: Vec<Finding> = findings
        .iter()
        .filter(|f| passes_threshold(f, policy))
        .cloned()
        .map(|mut f| {
            apply_action_override(&mut f, policy);
            f
        })
        .collect();

    let action = considered
        .iter()
        .map(|f| f.suggested_action)
        .max()
        .unwrap_or(policy.default_action);

    (action, considered)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use prompt_scanner_sdk::Severity;
    use uuid::Uuid;

    use super::*;

    fn finding(category: &str, confidence: f32, action: VerdictAction) -> Finding {
        Finding {
            detector: "test-detector".to_owned(),
            category: category.to_owned(),
            severity: Severity::Medium,
            confidence,
            span: None,
            suggested_action: action,
        }
    }

    fn policy() -> TenantPolicy {
        let mut p = TenantPolicy::default_for_tenant(Uuid::nil());
        p.thresholds.insert("secret.aws_key".to_owned(), 0.8);
        p
    }

    #[test]
    fn block_wins_over_warn() {
        let findings = vec![
            finding("pii.email", 0.9, VerdictAction::Warn),
            finding("secret.aws_key", 0.95, VerdictAction::Block),
        ];
        let (action, considered) = aggregate_verdict(&findings, &policy());
        assert_eq!(action, VerdictAction::Block);
        assert_eq!(considered.len(), 2);
    }

    #[test]
    fn low_confidence_finding_is_dropped() {
        let findings = vec![finding("secret.aws_key", 0.5, VerdictAction::Block)];
        let (action, considered) = aggregate_verdict(&findings, &policy());
        // Below the 0.8 threshold configured for this category → dropped →
        // falls back to the tenant's default_action (Allow).
        assert_eq!(action, VerdictAction::Allow);
        assert!(considered.is_empty());
    }

    #[test]
    fn no_findings_falls_back_to_default_action() {
        let mut p = policy();
        p.default_action = VerdictAction::Audit;
        let (action, considered) = aggregate_verdict(&[], &p);
        assert_eq!(action, VerdictAction::Audit);
        assert!(considered.is_empty());
    }

    /// The coupling that made the `always-allow-plugin` defect possible, pinned
    /// so it cannot come back silently.
    ///
    /// `default_action` is a **fallback**, not a floor: it applies only when no
    /// finding survives threshold filtering. It cannot be made a floor —
    /// `Block` is the maximum of the ordering, so a floor would block every
    /// request for a tenant whose default is `Block`. The consequence is that
    /// *any* finding consumes the fallback, including a meaningless
    /// zero-confidence `Allow`, which is why a backend with nothing to report
    /// must return an empty finding set rather than a placeholder.
    #[test]
    fn any_finding_suppresses_default_action_even_a_zero_confidence_allow() {
        let mut p = policy();
        p.default_action = VerdictAction::Block;

        // No findings: the tenant's fail-closed default stands.
        let (empty, considered) = aggregate_verdict(&[], &p);
        assert_eq!(
            empty,
            VerdictAction::Block,
            "an empty finding set must fall back to the tenant's default_action"
        );
        assert!(considered.is_empty());

        // One placeholder finding: the default is gone. This is the hazard.
        let (with_placeholder, considered) =
            aggregate_verdict(&[finding("none", 0.0, VerdictAction::Allow)], &p);
        assert_eq!(
            with_placeholder,
            VerdictAction::Allow,
            "a single zero-confidence Allow replaces a fail-closed default_action -- so a \
             backend that inspects nothing must report Findings(vec![]), not a placeholder"
        );
        assert_eq!(considered.len(), 1);
    }

    #[test]
    fn category_override_replaces_suggested_action() {
        let mut p = policy();
        p.category_action_overrides
            .insert("pii".to_owned(), VerdictAction::Allow);
        let findings = vec![finding("pii.email", 0.9, VerdictAction::Redact)];
        let (action, considered) = aggregate_verdict(&findings, &p);
        assert_eq!(action, VerdictAction::Allow);
        assert_eq!(considered[0].suggested_action, VerdictAction::Allow);
    }

    #[test]
    fn longest_matching_override_prefix_wins() {
        let mut p = policy();
        p.category_action_overrides
            .insert("pii".to_owned(), VerdictAction::Allow);
        p.category_action_overrides
            .insert("pii.email".to_owned(), VerdictAction::Block);
        let findings = vec![
            finding("pii.email", 0.9, VerdictAction::Redact),
            finding("pii.phone", 0.9, VerdictAction::Redact),
        ];
        let (action, considered) = aggregate_verdict(&findings, &p);
        assert_eq!(action, VerdictAction::Block);
        assert_eq!(
            considered
                .iter()
                .find(|f| f.category == "pii.email")
                .unwrap()
                .suggested_action,
            VerdictAction::Block
        );
        assert_eq!(
            considered
                .iter()
                .find(|f| f.category == "pii.phone")
                .unwrap()
                .suggested_action,
            VerdictAction::Allow
        );
    }

    #[test]
    fn no_override_leaves_suggested_action_untouched() {
        let findings = vec![finding("secret.aws_key", 0.95, VerdictAction::Block)];
        let (action, considered) = aggregate_verdict(&findings, &policy());
        assert_eq!(action, VerdictAction::Block);
        assert_eq!(considered[0].suggested_action, VerdictAction::Block);
    }

    #[test]
    fn precedence_ordering_is_block_redact_warn_audit_allow() {
        assert!(VerdictAction::Block > VerdictAction::Redact);
        assert!(VerdictAction::Redact > VerdictAction::Warn);
        assert!(VerdictAction::Warn > VerdictAction::Audit);
        assert!(VerdictAction::Audit > VerdictAction::Allow);
    }
}
