//! REST DTOs (serde + utoipa) for `demo-policy-console`.

use chrono::{DateTime, Utc};
use prompt_scanner_sdk::{ScanVerdict, VerdictAction};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::domain::DemoPolicyState;

/// Wire representation of [`VerdictAction`], restricted to the actions this
/// demo's radio buttons offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DemoVerdictActionDto {
    Allow,
    Audit,
    Warn,
    Redact,
    Block,
}

impl From<VerdictAction> for DemoVerdictActionDto {
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

impl From<DemoVerdictActionDto> for VerdictAction {
    fn from(a: DemoVerdictActionDto) -> Self {
        match a {
            DemoVerdictActionDto::Allow => Self::Allow,
            DemoVerdictActionDto::Audit => Self::Audit,
            DemoVerdictActionDto::Warn => Self::Warn,
            DemoVerdictActionDto::Redact => Self::Redact,
            DemoVerdictActionDto::Block => Self::Block,
        }
    }
}

/// `GET`/`PUT /demo-policy-console/v1/state` body.
#[derive(Debug, Clone)]
#[toolkit_macros::api_dto(request, response)]
pub struct DemoStateDto {
    pub pii_enabled: bool,
    pub pii_action: DemoVerdictActionDto,
}

impl From<DemoPolicyState> for DemoStateDto {
    fn from(s: DemoPolicyState) -> Self {
        Self {
            pii_enabled: s.pii_enabled,
            pii_action: s.pii_action.into(),
        }
    }
}

/// `GET /demo-policy-console/v1/audit` response entry — a reshaped
/// `ScanVerdict` for the UI's audit table (just the columns it renders).
#[derive(Debug, Clone)]
#[toolkit_macros::api_dto(response)]
pub struct AuditEntryDto {
    pub trace_id: Uuid,
    pub action: DemoVerdictActionDto,
    pub categories: Vec<String>,
    /// Detector(s) whose finding(s) actually decided `action` — i.e. every
    /// `Finding` whose (post-policy-override) `suggested_action` equals the
    /// aggregated verdict. A backend that only reported a lower-ranked
    /// finding (e.g. `regex-scanner` finding a `pii.email` while another
    /// backend's `Block` finding decided the verdict) is not "decisive" and
    /// won't appear here — see `domain::policy::aggregate_verdict` in the
    /// `prompt-scanner` gear for why `suggested_action` is already the
    /// post-override value by the time it's persisted.
    pub decisive_detectors: Vec<String>,
    pub created_at: DateTime<Utc>,
}

impl From<ScanVerdict> for AuditEntryDto {
    fn from(v: ScanVerdict) -> Self {
        let mut decisive_detectors: Vec<String> = v
            .findings
            .iter()
            .filter(|f| f.suggested_action == v.action)
            .map(|f| f.detector.clone())
            .collect();
        decisive_detectors.sort_unstable();
        decisive_detectors.dedup();

        Self {
            trace_id: v.trace_id,
            action: v.action.into(),
            categories: v.findings.into_iter().map(|f| f.category).collect(),
            decisive_detectors,
            created_at: v.created_at,
        }
    }
}
