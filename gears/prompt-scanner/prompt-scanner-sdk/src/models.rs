//! Transport-agnostic domain models for the prompt-scanner gear.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Kind of content submitted for scanning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    /// An outbound prompt sent to an LLM.
    Prompt,
    /// An inbound response returned by an LLM.
    Response,
    /// A file/blob upload.
    Upload,
    /// An MCP tool call (request side).
    McpToolCall,
    /// An MCP tool result (response side).
    McpToolResult,
}

impl ContentKind {
    /// Every content kind.
    pub const ALL: [Self; 5] = [
        Self::Prompt,
        Self::Response,
        Self::Upload,
        Self::McpToolCall,
        Self::McpToolResult,
    ];

    /// [`Self::ALL`] as a `Vec`: the default of `TenantPolicy::content_kinds`.
    #[must_use]
    pub fn all() -> Vec<Self> {
        Self::ALL.to_vec()
    }
}

/// A byte offset span within the scanned payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    /// Start offset (inclusive).
    pub start: u32,
    /// End offset (exclusive).
    pub end: u32,
}

/// Detector-reported severity for a finding.
///
/// Declaration order is significant: `derive(Ord)` ranks variants by
/// declaration order, so `Critical > High > Medium > Low`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Informational / low risk.
    Low,
    /// Moderate risk.
    Medium,
    /// Significant risk.
    High,
    /// Severe risk, should almost always block.
    Critical,
}

/// The gateway's verdict action for a scanned payload.
///
/// # Precedence
///
/// Declaration order is significant: `derive(Ord)` ranks variants by
/// declaration order, so the **verdict-aggregation precedence rule** used by
/// `Service::aggregate_verdict` is simply "take the maximum action across all
/// backend findings":
///
/// `Block > Redact > Warn > Audit > Allow`
///
/// This is a deliberately simple, documented policy (see `docs/DESIGN.md`
/// open questions) — a backend voting `Block` always wins over one voting
/// `Warn`, regardless of confidence/severity. More nuanced weighting
/// (confidence-adjusted precedence, per-tenant override of the ordering) is
/// left as future work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerdictAction {
    /// No issue found; allow the content through unchanged.
    #[default]
    Allow,
    /// Allow, but record the finding for audit trail purposes.
    Audit,
    /// Allow, but surface a warning to the caller.
    Warn,
    /// Allow, but redact the offending content first.
    Redact,
    /// Block the content entirely.
    Block,
}

/// A single backend's finding, either as returned by a `ScannerBackendClient`
/// or as recorded in an aggregated `ScanVerdict`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    /// Name/vendor of the backend that produced this finding.
    pub detector: String,
    /// Backend-specific category (e.g. `"pii.email"`, `"secret.aws_key"`).
    pub category: String,
    /// Backend-reported severity.
    pub severity: Severity,
    /// Backend-reported confidence, in `[0.0, 1.0]`.
    pub confidence: f32,
    /// Optional span within the payload where the finding was detected.
    pub span: Option<Span>,
    /// The action this backend suggests for this finding alone.
    pub suggested_action: VerdictAction,
}

/// A backend's outcome for a single `detect` call: zero or more findings, or
/// `Defer`.
///
/// `Findings` carries *every* match a backend found in one scan (a message
/// can legitimately contain more than one distinct piece of PII/secret
/// material) — not just the single highest-priority one. An empty vec means
/// "no issue found," the same as every real PII/DLP scanner's API shape
/// (Presidio, AWS Comprehend PII, Google Cloud DLP all return a list per
/// scan, for the same reason: real text routinely contains multiple
/// distinct sensitive items, and a scanner that only reports its "best"
/// match under-reports the rest).
///
/// `Defer` is distinct from an error: a backend that errors is treated as
/// "did not produce a finding" by `Service::aggregate_verdict` (see
/// `docs/DESIGN.md`), but a backend that defers has its work pushed onto the
/// existing `scan_async` job queue instead of being dropped. See
/// `gears/onnx-runner/docs/DESIGN.md`'s "Async deferral for oversized
/// inputs" for the full mechanism.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BackendOutcome {
    /// Every finding this backend produced for the scanned payload — may be
    /// empty.
    Findings(Vec<Finding>),
    /// The backend is deferring this input — too expensive to evaluate
    /// synchronously (e.g. the payload exceeds the backend's own configured
    /// size threshold). The orchestrator queues this backend's contribution
    /// for background processing under the scan's existing `trace_id`.
    Defer,
}

/// Input passed to a scanner backend's `detect` call.
///
/// A narrowed view of `ScanRequest` — backends do not need to know the
/// connector id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScanInput {
    /// Tenant the content belongs to.
    pub tenant_id: Uuid,
    /// Kind of content being scanned.
    pub content_kind: ContentKind,
    /// The raw payload to scan.
    pub payload: String,
    /// Free-form metadata (e.g. model name, source IP).
    pub metadata: HashMap<String, String>,
}

/// A raw file attachment submitted for scanning under
/// `content_kind: Upload`.
///
/// `prompt-scanner` extracts text from this via `file-parser` (see
/// `Service::scan_with_trace_id`) before running the detector fan-out — the
/// connector (`forward-proxy`'s interceptor plugin, `llm-gateway` hook
/// plugin, endpoint agent, ...) sends raw bytes once, rather than calling `file-parser`
/// itself and forwarding pre-extracted text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    /// Original filename, if known — helps `file-parser` pick a backend
    /// when `mime_type` is absent or generic.
    #[serde(default)]
    pub filename: Option<String>,
    /// MIME type, if known.
    #[serde(default)]
    pub mime_type: Option<String>,
    /// Base64-encoded (standard alphabet, padded) raw file bytes.
    pub data_base64: String,
}

/// A request to scan a piece of content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScanRequest {
    /// Tenant the content belongs to.
    pub tenant_id: Uuid,
    /// Identifier of the connector submitting the request (e.g.
    /// `"llm-gateway"`, `"endpoint-agent"`).
    pub connector_id: String,
    /// Kind of content being scanned.
    pub content_kind: ContentKind,
    /// The raw payload to scan. Ignored when `attachment` is present (it may
    /// be left empty in that case): the effective payload is whatever text
    /// `file-parser` extracts from `attachment`.
    #[serde(default)]
    pub payload: String,
    /// Free-form metadata (e.g. model name, source IP).
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    /// Only meaningful when `content_kind == ContentKind::Upload`: a raw
    /// file attachment to extract text from via `file-parser` before
    /// scanning. See [`Attachment`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment: Option<Attachment>,
}

impl ScanRequest {
    /// Narrow this request into the input passed to scanner backends.
    ///
    /// Callers extracting text from an `attachment` (see `Service` in the
    /// `prompt-scanner` gear) should replace `payload` with the extracted text
    /// *before* calling this, so backends always see plain text regardless
    /// of `content_kind`.
    #[must_use]
    pub fn to_scan_input(&self) -> ScanInput {
        ScanInput {
            tenant_id: self.tenant_id,
            content_kind: self.content_kind,
            payload: self.payload.clone(),
            metadata: self.metadata.clone(),
        }
    }
}

/// Aggregated verdict for a scanned payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScanVerdict {
    /// Unique trace identifier for this scan (used for later lookup / audit).
    pub trace_id: Uuid,
    /// Tenant the scan was performed for.
    pub tenant_id: Uuid,
    /// The aggregated action.
    pub action: VerdictAction,
    /// All findings from all detectors that responded.
    pub findings: Vec<Finding>,
    /// Redacted payload, present when `action == Redact`.
    pub redacted_payload: Option<String>,
    /// When the scan was performed.
    pub created_at: DateTime<Utc>,
}

/// Status of an asynchronously-processed scan job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanJobStatus {
    /// Queued, not yet picked up by the background worker.
    Queued,
    /// Currently being scanned.
    Running,
    /// Finished; the verdict is available via `get_verdict`.
    Completed,
    /// Failed; no verdict was produced.
    Failed,
}

/// Handle returned by `scan_async` for polling job status / fetching the
/// eventual verdict via `trace_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanJobHandle {
    /// Unique job identifier.
    pub job_id: Uuid,
    /// The trace id the eventual `ScanVerdict` will be recorded under.
    pub trace_id: Uuid,
    /// Current job status.
    pub status: ScanJobStatus,
}

/// Per-tenant scanner backend configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantPolicy {
    /// Tenant this policy applies to.
    pub tenant_id: Uuid,
    /// Vendor identifiers of scanner backend plugins enabled for this
    /// tenant.
    ///
    /// Matched against the `vendor` field of registered
    /// `ScannerBackendSpecV1` instances in `types-registry`.
    #[serde(default)]
    pub enabled_backends: Vec<String>,
    /// Subset of `enabled_backends` that must respond for the scan to
    /// succeed. If a required backend times out or errors, the scan fails
    /// closed (`VerdictAction::Block`) rather than degrading to "unscored".
    #[serde(default)]
    pub required_backends: Vec<String>,
    /// Per-category confidence thresholds below which a finding is ignored
    /// during aggregation. Keyed by `Finding::category`.
    #[serde(default)]
    pub thresholds: HashMap<String, f32>,
    /// Action to take when no finding exceeds its threshold.
    #[serde(default)]
    pub default_action: VerdictAction,
    /// Overrides a finding's own `suggested_action` when its category
    /// starts with a configured key (prefix match, longest match wins).
    /// Applied by `Service::aggregate_verdict` before taking the max action
    /// across findings. Empty by default: detectors' own suggested actions
    /// are authoritative, exactly as before this field existed.
    ///
    /// This is what lets a policy change *what happens* on a finding (not
    /// just whether it counts, per `thresholds`) — e.g. mapping every
    /// `"pii."`-prefixed category to `Allow` to effectively turn PII
    /// detection's effect off without disabling the backends that produce
    /// those findings (which may also report non-PII categories).
    #[serde(default)]
    pub category_action_overrides: HashMap<String, VerdictAction>,
    /// Content kinds this policy governs. A scan of any other kind runs no
    /// backends and is allowed; its record is still saved. Absent means
    /// every kind, as before this field existed.
    #[serde(default = "ContentKind::all")]
    pub content_kinds: Vec<ContentKind>,
}

impl TenantPolicy {
    /// A permissive default policy: no backends enabled, default action
    /// `Allow`, every content kind. Used when a tenant has not yet
    /// configured a policy.
    #[must_use]
    pub fn default_for_tenant(tenant_id: Uuid) -> Self {
        Self {
            tenant_id,
            enabled_backends: Vec::new(),
            required_backends: Vec::new(),
            thresholds: HashMap::new(),
            default_action: VerdictAction::Allow,
            category_action_overrides: HashMap::new(),
            content_kinds: ContentKind::all(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_content_kinds_means_every_kind() {
        let policy: TenantPolicy = serde_json::from_value(serde_json::json!({
            "tenant_id": Uuid::nil(),
        }))
        .unwrap();
        assert_eq!(policy.content_kinds, ContentKind::ALL);

        let policy: TenantPolicy = serde_json::from_value(serde_json::json!({
            "tenant_id": Uuid::nil(),
            "content_kinds": ["prompt", "mcp_tool_call"],
        }))
        .unwrap();
        assert_eq!(
            policy.content_kinds,
            [ContentKind::Prompt, ContentKind::McpToolCall]
        );
    }
}
