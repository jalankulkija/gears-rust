//! `OData` filter field definitions for `scan_record` audit search
//! (`GET /prompt-scanner/v1/scans`).

use chrono::{DateTime, Utc};
use toolkit_odata_macros::ODataFilterable;
use uuid::Uuid;

/// Filterable fields schema for scan records.
///
/// Field names match the wire format used by `$filter` / `$orderby`.
#[derive(ODataFilterable)]
pub struct ScanRecordQuery {
    #[odata(filter(kind = "Uuid"))]
    pub trace_id: Uuid,

    #[odata(filter(kind = "Uuid"))]
    pub tenant_id: Uuid,

    #[odata(filter(kind = "String"))]
    pub connector_id: String,

    #[odata(filter(kind = "DateTimeUtc"))]
    pub created_at: DateTime<Utc>,
}

/// Generated filter field enum for [`ScanRecordQuery`].
pub use ScanRecordQueryFilterField as ScanRecordFilterField;
