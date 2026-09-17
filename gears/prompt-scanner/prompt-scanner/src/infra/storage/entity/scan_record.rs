//! `SeaORM` entity for the `scan_records` table — the audit trail required
//! by `docs/DESIGN.md`.

use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use toolkit_db_macros::Scopable;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Scopable)]
#[sea_orm(table_name = "scan_records")]
#[secure(tenant_col = "tenant_id", resource_col = "trace_id", no_owner, no_type)]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub trace_id: Uuid,
    pub tenant_id: Uuid,
    pub connector_id: String,
    /// Serialized `prompt_scanner_sdk::ContentKind` (`snake_case` tag).
    pub content_kind: String,
    /// Serialized `prompt_scanner_sdk::VerdictAction` (`snake_case` tag).
    pub action: String,
    /// Serialized `Vec<prompt_scanner_sdk::Finding>`.
    ///
    /// Stored as `jsonb` on Postgres and `TEXT` on `SQLite` (`SeaORM`'s
    /// `Json` column type maps transparently to both), matching the DDL in
    /// `migrations::m20260805_000001_initial`.
    #[sea_orm(column_type = "Json")]
    pub findings: Json,
    pub redacted_payload: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
