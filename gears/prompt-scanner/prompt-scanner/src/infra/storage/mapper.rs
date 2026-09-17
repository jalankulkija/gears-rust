//! Entity <-> domain model conversions.
//!
//! `ContentKind` / `VerdictAction` are stored as their serde `snake_case`
//! tag strings (via `serde_json`) rather than a hand-rolled match, so the
//! wire format and the DB format can never drift independently.

use prompt_scanner_sdk::{ContentKind, VerdictAction};

use crate::domain::error::DomainError;
use crate::domain::repos::ScanRecord;
use crate::infra::storage::entity;

fn content_kind_to_str(kind: ContentKind) -> Result<String, DomainError> {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .ok_or_else(|| DomainError::database("failed to serialize content_kind"))
}

fn content_kind_from_str(s: &str) -> Result<ContentKind, DomainError> {
    serde_json::from_value(serde_json::Value::String(s.to_owned()))
        .map_err(|e| DomainError::database(format!("invalid content_kind '{s}': {e}")))
}

pub(crate) fn action_to_str(action: VerdictAction) -> Result<String, DomainError> {
    serde_json::to_value(action)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .ok_or_else(|| DomainError::database("failed to serialize action"))
}

fn action_from_str(s: &str) -> Result<VerdictAction, DomainError> {
    serde_json::from_value(serde_json::Value::String(s.to_owned()))
        .map_err(|e| DomainError::database(format!("invalid action '{s}': {e}")))
}

/// Convert a `ScanRecord` domain aggregate into a `scan_record` `ActiveModel`
/// column set (as plain values; callers wrap in `Set(..)`).
pub(crate) fn scan_record_to_row(
    record: &ScanRecord,
) -> Result<(String, String, serde_json::Value), DomainError> {
    let content_kind = content_kind_to_str(record.content_kind)?;
    let action = action_to_str(record.action)?;
    let findings = serde_json::to_value(&record.findings)
        .map_err(|e| DomainError::database(format!("failed to serialize findings: {e}")))?;
    Ok((content_kind, action, findings))
}

/// Convert a `scan_records` row into the domain `ScanRecord` aggregate.
pub(crate) fn scan_record_from_model(
    m: entity::scan_record::Model,
) -> Result<ScanRecord, DomainError> {
    Ok(ScanRecord {
        trace_id: m.trace_id,
        tenant_id: m.tenant_id,
        connector_id: m.connector_id,
        content_kind: content_kind_from_str(&m.content_kind)?,
        action: action_from_str(&m.action)?,
        findings: serde_json::from_value(m.findings)
            .map_err(|e| DomainError::database(format!("failed to deserialize findings: {e}")))?,
        redacted_payload: m.redacted_payload,
        created_at: m.created_at,
    })
}
