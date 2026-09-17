//! `SeaORM`-backed implementations of the domain repository traits.

use async_trait::async_trait;
use prompt_scanner_sdk::odata::ScanRecordFilterField;
use prompt_scanner_sdk::{Finding, VerdictAction};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, Set};
use toolkit_db::odata::LimitCfg;
use toolkit_db::odata::sea_orm_filter::{PaginateOdataTryError, paginate_odata_try};
use toolkit_db::secure::{DBRunner, SecureEntityExt, secure_insert, secure_update_with_scope};
use toolkit_odata::{ODataQuery, Page, SortDir};
use toolkit_security::AccessScope;
use uuid::Uuid;

use crate::domain::error::DomainError;
use crate::domain::repos::{ScanRecord, ScanRecordRepository};
use crate::infra::storage::entity::scan_record::{
    ActiveModel as ScanRecordAM, Column as ScanRecordColumn, Entity as ScanRecordEntity,
};
use crate::infra::storage::mapper;
use crate::infra::storage::odata_mapper::ScanRecordODataMapper;

fn db_err(e: impl std::fmt::Display) -> DomainError {
    DomainError::database(e.to_string())
}

/// `SeaORM`-backed implementation of [`ScanRecordRepository`].
#[derive(Clone)]
pub struct OrmScanRecordRepository {
    limit_cfg: LimitCfg,
}

impl OrmScanRecordRepository {
    #[must_use]
    pub fn new(limit_cfg: LimitCfg) -> Self {
        Self { limit_cfg }
    }
}

#[async_trait]
impl ScanRecordRepository for OrmScanRecordRepository {
    async fn get<C: DBRunner>(
        &self,
        conn: &C,
        scope: &AccessScope,
        trace_id: Uuid,
    ) -> Result<Option<ScanRecord>, DomainError> {
        let found = ScanRecordEntity::find()
            .filter(sea_orm::Condition::all().add(ScanRecordColumn::TraceId.eq(trace_id)))
            .secure()
            .scope_with(scope)
            .one(conn)
            .await
            .map_err(db_err)?;

        found.map(mapper::scan_record_from_model).transpose()
    }

    async fn list_page<C: DBRunner>(
        &self,
        conn: &C,
        scope: &AccessScope,
        query: &ODataQuery,
    ) -> Result<Page<ScanRecord>, DomainError> {
        let base_query = ScanRecordEntity::find().secure().scope_with(scope);

        paginate_odata_try::<ScanRecordFilterField, ScanRecordODataMapper, _, _, _, _, _>(
            base_query,
            conn,
            query,
            ("trace_id", SortDir::Desc),
            self.limit_cfg,
            mapper::scan_record_from_model,
        )
        .await
        .map_err(|e| match e {
            PaginateOdataTryError::OData(od) => db_err(od),
            PaginateOdataTryError::MapError(domain_err) => domain_err,
        })
    }

    async fn create<C: DBRunner>(
        &self,
        conn: &C,
        scope: &AccessScope,
        record: ScanRecord,
    ) -> Result<ScanRecord, DomainError> {
        let (content_kind, action, findings) = mapper::scan_record_to_row(&record)?;

        let am = ScanRecordAM {
            trace_id: Set(record.trace_id),
            tenant_id: Set(record.tenant_id),
            connector_id: Set(record.connector_id.clone()),
            content_kind: Set(content_kind),
            action: Set(action),
            findings: Set(findings),
            redacted_payload: Set(record.redacted_payload.clone()),
            created_at: Set(record.created_at),
        };

        secure_insert::<ScanRecordEntity>(am, scope, conn)
            .await
            .map_err(db_err)?;
        Ok(record)
    }

    async fn update_findings<C: DBRunner>(
        &self,
        conn: &C,
        scope: &AccessScope,
        trace_id: Uuid,
        action: VerdictAction,
        findings: Vec<Finding>,
    ) -> Result<(), DomainError> {
        let action_str = mapper::action_to_str(action)?;
        let findings_json = serde_json::to_value(&findings)
            .map_err(|e| DomainError::database(format!("failed to serialize findings: {e}")))?;

        let am = ScanRecordAM {
            trace_id: sea_orm::ActiveValue::Unchanged(trace_id),
            action: Set(action_str),
            findings: Set(findings_json),
            ..Default::default()
        };

        secure_update_with_scope::<ScanRecordEntity>(am, scope, trace_id, conn)
            .await
            .map_err(db_err)?;
        Ok(())
    }
}
