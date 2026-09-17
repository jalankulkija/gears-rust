//! REST DTOs (serde + utoipa) for the prompt-scanner gear.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use prompt_scanner_sdk::{
    Attachment, ContentKind, Finding, ScanJobHandle, ScanJobStatus, ScanRequest, ScanVerdict,
    Severity, Span, TenantPolicy, VerdictAction,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// Wire representation of [`ContentKind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContentKindDto {
    Prompt,
    Response,
    Upload,
    McpToolCall,
    McpToolResult,
}

impl From<ContentKind> for ContentKindDto {
    fn from(k: ContentKind) -> Self {
        match k {
            ContentKind::Prompt => Self::Prompt,
            ContentKind::Response => Self::Response,
            ContentKind::Upload => Self::Upload,
            ContentKind::McpToolCall => Self::McpToolCall,
            ContentKind::McpToolResult => Self::McpToolResult,
        }
    }
}

impl From<ContentKindDto> for ContentKind {
    fn from(k: ContentKindDto) -> Self {
        match k {
            ContentKindDto::Prompt => Self::Prompt,
            ContentKindDto::Response => Self::Response,
            ContentKindDto::Upload => Self::Upload,
            ContentKindDto::McpToolCall => Self::McpToolCall,
            ContentKindDto::McpToolResult => Self::McpToolResult,
        }
    }
}

/// Wire representation of [`VerdictAction`].
///
/// Ordered the same as the SDK's `VerdictAction` (see
/// `prompt_scanner_sdk::models::VerdictAction` docs for the precedence rule
/// this ordering encodes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum VerdictActionDto {
    Allow,
    Audit,
    Warn,
    Redact,
    Block,
}

impl From<VerdictAction> for VerdictActionDto {
    fn from(a: VerdictAction) -> Self {
        match a {
            VerdictAction::Allow => Self::Allow,
            VerdictAction::Audit => Self::Audit,
            VerdictAction::Warn => Self::Warn,
            VerdictAction::Redact => Self::Redact,
            VerdictAction::Block => Self::Block,
        }
    }
}

impl From<VerdictActionDto> for VerdictAction {
    fn from(a: VerdictActionDto) -> Self {
        match a {
            VerdictActionDto::Allow => Self::Allow,
            VerdictActionDto::Audit => Self::Audit,
            VerdictActionDto::Warn => Self::Warn,
            VerdictActionDto::Redact => Self::Redact,
            VerdictActionDto::Block => Self::Block,
        }
    }
}

/// Wire representation of [`Severity`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SeverityDto {
    Low,
    Medium,
    High,
    Critical,
}

impl From<Severity> for SeverityDto {
    fn from(s: Severity) -> Self {
        match s {
            Severity::Low => Self::Low,
            Severity::Medium => Self::Medium,
            Severity::High => Self::High,
            Severity::Critical => Self::Critical,
        }
    }
}

/// Wire representation of [`Span`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SpanDto {
    pub start: u32,
    pub end: u32,
}

impl From<Span> for SpanDto {
    fn from(s: Span) -> Self {
        Self {
            start: s.start,
            end: s.end,
        }
    }
}

/// Wire representation of a detector [`Finding`].
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FindingDto {
    pub detector: String,
    pub category: String,
    pub severity: SeverityDto,
    pub confidence: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<SpanDto>,
    pub suggested_action: VerdictActionDto,
}

impl From<Finding> for FindingDto {
    fn from(f: Finding) -> Self {
        Self {
            detector: f.detector,
            category: f.category,
            severity: f.severity.into(),
            confidence: f.confidence,
            span: f.span.map(Into::into),
            suggested_action: f.suggested_action.into(),
        }
    }
}

/// Wire representation of a raw [`Attachment`] submitted under
/// `content_kind: "upload"`. `prompt-scanner` extracts text from it via
/// `file-parser` before scanning — see `domain::service::Service::
/// resolve_effective_payload`.
#[derive(Debug, Clone)]
#[toolkit_macros::api_dto(request)]
pub struct AttachmentDto {
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
    /// Base64-encoded (standard alphabet, padded) raw file bytes.
    pub data_base64: String,
}

impl From<AttachmentDto> for Attachment {
    fn from(dto: AttachmentDto) -> Self {
        Self {
            filename: dto.filename,
            mime_type: dto.mime_type,
            data_base64: dto.data_base64,
        }
    }
}

/// `POST /prompt-scanner/v1/scans` (and `/scans/async`) request body.
#[derive(Debug, Clone)]
#[toolkit_macros::api_dto(request)]
pub struct ScanRequestDto {
    pub tenant_id: Uuid,
    pub connector_id: String,
    pub content_kind: ContentKindDto,
    /// Ignored when `attachment` is set (may be left empty in that case).
    #[serde(default)]
    pub payload: String,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    /// Only meaningful when `content_kind == "upload"`. See [`AttachmentDto`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment: Option<AttachmentDto>,
}

impl From<ScanRequestDto> for ScanRequest {
    fn from(dto: ScanRequestDto) -> Self {
        Self {
            tenant_id: dto.tenant_id,
            connector_id: dto.connector_id,
            content_kind: dto.content_kind.into(),
            payload: dto.payload,
            metadata: dto.metadata,
            attachment: dto.attachment.map(Into::into),
        }
    }
}

/// `POST /prompt-scanner/v1/scans` response body.
#[derive(Debug, Clone)]
#[toolkit_macros::api_dto(response)]
pub struct ScanVerdictDto {
    pub trace_id: Uuid,
    pub tenant_id: Uuid,
    pub action: VerdictActionDto,
    pub findings: Vec<FindingDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redacted_payload: Option<String>,
    #[schema(format = "date-time")]
    pub created_at: DateTime<Utc>,
}

impl From<ScanVerdict> for ScanVerdictDto {
    fn from(v: ScanVerdict) -> Self {
        Self {
            trace_id: v.trace_id,
            tenant_id: v.tenant_id,
            action: v.action.into(),
            findings: v.findings.into_iter().map(Into::into).collect(),
            redacted_payload: v.redacted_payload,
            created_at: v.created_at,
        }
    }
}

/// Wire representation of [`ScanJobStatus`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScanJobStatusDto {
    Queued,
    Running,
    Completed,
    Failed,
}

impl From<ScanJobStatus> for ScanJobStatusDto {
    fn from(s: ScanJobStatus) -> Self {
        match s {
            ScanJobStatus::Queued => Self::Queued,
            ScanJobStatus::Running => Self::Running,
            ScanJobStatus::Completed => Self::Completed,
            ScanJobStatus::Failed => Self::Failed,
        }
    }
}

/// `POST /prompt-scanner/v1/scans/async` response body.
#[derive(Debug, Clone)]
#[toolkit_macros::api_dto(response)]
pub struct ScanJobHandleDto {
    pub job_id: Uuid,
    pub trace_id: Uuid,
    pub status: ScanJobStatusDto,
}

impl From<ScanJobHandle> for ScanJobHandleDto {
    fn from(h: ScanJobHandle) -> Self {
        Self {
            job_id: h.job_id,
            trace_id: h.trace_id,
            status: h.status.into(),
        }
    }
}

/// `GET /prompt-scanner/v1/policy` response / `PUT` request body.
#[derive(Debug, Clone)]
#[toolkit_macros::api_dto(request, response)]
pub struct TenantPolicyDto {
    pub tenant_id: Uuid,
    #[serde(default)]
    pub enabled_backends: Vec<String>,
    #[serde(default)]
    pub required_backends: Vec<String>,
    #[serde(default)]
    pub thresholds: HashMap<String, f32>,
    pub default_action: VerdictActionDto,
    /// Category-prefix overrides for a finding's suggested action (e.g.
    /// `"pii"` -> `redact`). See `TenantPolicy::category_action_overrides`.
    #[serde(default)]
    pub category_action_overrides: HashMap<String, VerdictActionDto>,
    /// Content kinds this policy governs; absent means every kind. See
    /// `TenantPolicy::content_kinds`.
    #[serde(default = "all_content_kinds")]
    pub content_kinds: Vec<ContentKindDto>,
}

fn all_content_kinds() -> Vec<ContentKindDto> {
    ContentKind::ALL.into_iter().map(Into::into).collect()
}

impl From<TenantPolicy> for TenantPolicyDto {
    fn from(p: TenantPolicy) -> Self {
        Self {
            tenant_id: p.tenant_id,
            enabled_backends: p.enabled_backends,
            required_backends: p.required_backends,
            thresholds: p.thresholds,
            default_action: p.default_action.into(),
            category_action_overrides: p
                .category_action_overrides
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
            content_kinds: p.content_kinds.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<TenantPolicyDto> for TenantPolicy {
    fn from(dto: TenantPolicyDto) -> Self {
        Self {
            tenant_id: dto.tenant_id,
            enabled_backends: dto.enabled_backends,
            required_backends: dto.required_backends,
            thresholds: dto.thresholds,
            default_action: dto.default_action.into(),
            category_action_overrides: dto
                .category_action_overrides
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
            content_kinds: dto.content_kinds.into_iter().map(Into::into).collect(),
        }
    }
}

/// `GET /prompt-scanner/v1/backends` response entry.
#[derive(Debug, Clone)]
#[toolkit_macros::api_dto(response)]
pub struct BackendInstanceDto {
    /// GTS instance id of the scanner backend plugin.
    pub gts_id: String,
    /// Vendor identifier (matched against `TenantPolicy.enabled_backends`).
    pub vendor: String,
    /// Selection priority as registered (informational only — prompt-scanner
    /// fans out to all matching instances rather than picking by priority).
    pub priority: i16,
    /// Whether a scoped `ClientHub` entry is currently registered for this
    /// instance (i.e. whether the gateway could actually call it right now).
    pub available: bool,
}
