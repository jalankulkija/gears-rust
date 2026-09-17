//! Gear configuration — the pattern set is data, not code.
//!
//! Every detection this backend performs comes from [`PatternConfig`] entries
//! in gear config, compiled once at `init` (see `crate::gear`). The contract is
//! `gears/prompt-scanner/docs/prompt-scanner-interface-reference.md` §4.2.
//!
//! # Why `patterns` defaults to a built-in set rather than empty
//!
//! An absent `patterns` key yields [`default_patterns`] — the seventeen
//! patterns this backend shipped with — so a deployment that does not mention
//! patterns keeps detecting. An *explicitly* empty list (`patterns: []`) is
//! honoured and disables detection, per the contract. serde distinguishes the
//! two, and the distinction matters: defaulting to empty would let
//! `config: {}` silently turn a security backend into a no-op.

use prompt_scanner_sdk::VerdictAction;
use serde::{Deserialize, Serialize};

/// Configuration for the `regex-scanner` gear.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegexScannerConfig {
    #[serde(default = "default_vendor")]
    pub vendor: String,
    #[serde(default = "default_priority")]
    pub priority: i16,
    /// Minimum Shannon entropy (bits/char) a value must carry for a pattern
    /// using the `entropy` validator to report it.
    #[serde(default = "default_entropy_threshold")]
    pub entropy_threshold: f64,
    /// Patterns to run. Absent means [`default_patterns`]; an explicit empty
    /// list disables detection.
    #[serde(default = "default_patterns")]
    pub patterns: Vec<PatternConfig>,
}

/// One detection pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatternConfig {
    /// Reported verbatim as `Finding::category`. Keep it dotted
    /// (`family.thing`) so a tenant's `category_action_overrides` can target a
    /// whole family by prefix.
    pub category: String,
    /// Rust `regex` crate syntax — no backreferences, no lookaround.
    pub regex: String,
    /// Confidence reported on the finding. Defaults to [`Tier::Pattern`].
    #[serde(default)]
    pub tier: Tier,
    /// The finding's `suggested_action`, overridable per tenant.
    pub action: VerdictAction,
    /// Post-match check; a match that fails it is dropped.
    #[serde(default)]
    pub validator: Option<Validator>,
    /// Capture group the validator inspects. The reported span always covers
    /// group 0.
    #[serde(default)]
    pub capture: usize,
}

/// How a match was confirmed, cheapest to most certain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// A bare regex match with no further verification.
    #[default]
    Pattern,
    /// A regex match plus a structural check short of a full checksum.
    Structural,
    /// A regex match confirmed by a checksum or parser.
    Validated,
}

impl Tier {
    /// Numeric confidence reported on the resulting `Finding`.
    #[must_use]
    pub const fn confidence(self) -> f32 {
        match self {
            Self::Pattern => 0.5,
            Self::Structural => 0.75,
            Self::Validated => 0.95,
        }
    }
}

/// Post-match checks a pattern may attach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Validator {
    /// Luhn checksum over the digits of the match, which must number 13-19
    /// once separators are stripped (the PAN lengths the major networks use).
    Luhn,
    /// IBAN mod-97-10 (ISO 7064), whitespace stripped first.
    Iban,
    /// Parses as an IPv4 address.
    Ipv4,
    /// Parses as an IPv6 address.
    Ipv6,
    /// Three base64url segments whose header decodes to something JSON-shaped.
    /// A shape check, not signature verification.
    Jwt,
    /// Shannon entropy at or above the configured `entropy_threshold`.
    Entropy,
}

fn default_vendor() -> String {
    "cf.builtin.regex_scanner".to_owned()
}

const fn default_priority() -> i16 {
    500
}

const fn default_entropy_threshold() -> f64 {
    3.0
}

impl Default for RegexScannerConfig {
    fn default() -> Self {
        Self {
            vendor: default_vendor(),
            priority: default_priority(),
            entropy_threshold: default_entropy_threshold(),
            patterns: default_patterns(),
        }
    }
}

/// Terse constructor for the built-in table below.
fn p(
    category: &str,
    regex: &str,
    tier: Tier,
    action: VerdictAction,
    validator: Option<Validator>,
    capture: usize,
) -> PatternConfig {
    PatternConfig {
        category: category.to_owned(),
        regex: regex.to_owned(),
        tier,
        action,
        validator,
        capture,
    }
}

/// The patterns this backend detects when config names none.
///
/// Deliberately the same set, tiers and actions the gear shipped with, so
/// making patterns configurable changed no behaviour for a deployment that
/// does not configure them.
#[must_use]
pub fn default_patterns() -> Vec<PatternConfig> {
    use Tier::{Pattern, Structural, Validated};
    use Validator::{Entropy, Iban, Ipv4, Ipv6, Jwt, Luhn};
    use VerdictAction::{Block, Redact, Warn};

    vec![
        // -- contact ---------------------------------------------------------
        p(
            "pii.email",
            r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}",
            Pattern,
            Redact,
            None,
            0,
        ),
        p(
            "pii.url",
            r#"https?://[^\s<>"']+"#,
            Pattern,
            Redact,
            None,
            0,
        ),
        p(
            "pii.ip_address",
            r"\b(?:\d{1,3}\.){3}\d{1,3}\b",
            Validated,
            Redact,
            Some(Ipv4),
            0,
        ),
        p(
            "pii.ip_address",
            r"\b(?:[A-Fa-f0-9]{0,4}:){2,7}[A-Fa-f0-9]{0,4}\b",
            Validated,
            Redact,
            Some(Ipv6),
            0,
        ),
        // Pattern-tier on purpose: no checksum exists for phone numbers and
        // the shape over-matches generic digit runs.
        p(
            "pii.phone",
            r"\+?\d{1,3}?[\s.-]?\(?\d{2,4}\)?[\s.-]?\d{3,4}[\s.-]?\d{3,4}",
            Pattern,
            Redact,
            None,
            0,
        ),
        // -- financial -------------------------------------------------------
        // A Luhn-valid PAN is high-confidence secret material, not merely PII.
        p(
            "financial.credit_card",
            r"\b(?:\d[ -]?){12,18}\d\b",
            Validated,
            Block,
            Some(Luhn),
            0,
        ),
        p(
            "financial.iban",
            r"\b[A-Z]{2}\d{2}[A-Z0-9]{11,30}\b",
            Validated,
            Redact,
            Some(Iban),
            0,
        ),
        // -- vendor credentials ----------------------------------------------
        p(
            "secret.aws_key",
            r"\bAKIA[0-9A-Z]{16}\b",
            Structural,
            Block,
            None,
            0,
        ),
        p(
            "secret.github_token",
            r"\bgh[pousr]_[A-Za-z0-9]{36}\b",
            Structural,
            Block,
            None,
            0,
        ),
        p(
            "secret.openai_key",
            r"\bsk-[A-Za-z0-9]{20,}\b",
            Structural,
            Block,
            None,
            0,
        ),
        p(
            "secret.anthropic_key",
            r"\bsk-ant-[A-Za-z0-9_-]{20,}\b",
            Structural,
            Block,
            None,
            0,
        ),
        p(
            "secret.slack_token",
            r"\bxox[baprs]-[A-Za-z0-9-]{10,}\b",
            Structural,
            Block,
            None,
            0,
        ),
        p(
            "secret.stripe_key",
            r"\bsk_live_[A-Za-z0-9]{24,}\b",
            Structural,
            Block,
            None,
            0,
        ),
        // -- secret shapes ---------------------------------------------------
        p(
            "secret.jwt",
            r"\b[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b",
            Structural,
            Block,
            Some(Jwt),
            0,
        ),
        p(
            "secret.private_key",
            r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----",
            Structural,
            Block,
            None,
            0,
        ),
        p(
            "secret.connection_string",
            r"\b[A-Za-z][A-Za-z0-9+.-]*://[^\s:/@]+:[^\s:/@]+@[^\s/]+",
            Pattern,
            Block,
            None,
            0,
        ),
        // `capture: 1` — the validator inspects the value, the span covers the
        // whole `key=value` match.
        p(
            "secret.generic",
            r#"(?i)\b(?:password|passwd|secret|api[_-]?key|access[_-]?token|token)\s*[:=]\s*["']?([A-Za-z0-9+/_.-]{8,})["']?"#,
            Pattern,
            Warn,
            Some(Entropy),
            1,
        ),
    ]
}
