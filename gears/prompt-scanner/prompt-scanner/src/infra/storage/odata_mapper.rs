//! Infrastructure layer mapping from type-safe `FilterNode` (via
//! `ScanRecordFilterField`) to `SeaORM` columns for `GET /prompt-scanner/v1/scans`.

use toolkit_db::odata::sea_orm_filter::{FieldToColumn, ODataFieldMapping};

use prompt_scanner_sdk::odata::ScanRecordFilterField;

use crate::infra::storage::entity::scan_record::{Column, Entity, Model};

/// `OData` mapper for `scan_records`.
pub struct ScanRecordODataMapper;

impl FieldToColumn<ScanRecordFilterField> for ScanRecordODataMapper {
    type Column = Column;

    fn map_field(field: ScanRecordFilterField) -> Column {
        match field {
            ScanRecordFilterField::TraceId => Column::TraceId,
            ScanRecordFilterField::TenantId => Column::TenantId,
            ScanRecordFilterField::ConnectorId => Column::ConnectorId,
            ScanRecordFilterField::CreatedAt => Column::CreatedAt,
        }
    }
}

impl ODataFieldMapping<ScanRecordFilterField> for ScanRecordODataMapper {
    type Entity = Entity;

    fn extract_cursor_value(model: &Model, field: ScanRecordFilterField) -> sea_orm::Value {
        match field {
            ScanRecordFilterField::TraceId => sea_orm::Value::Uuid(Some(model.trace_id)),
            ScanRecordFilterField::TenantId => sea_orm::Value::Uuid(Some(model.tenant_id)),
            ScanRecordFilterField::ConnectorId => {
                sea_orm::Value::String(Some(model.connector_id.clone()))
            }
            ScanRecordFilterField::CreatedAt => {
                sea_orm::Value::ChronoDateTimeUtc(Some(model.created_at))
            }
        }
    }
}
