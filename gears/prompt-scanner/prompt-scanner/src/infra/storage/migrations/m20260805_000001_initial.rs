use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, DbErr};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        let conn = manager.get_connection();

        let sql = match backend {
            sea_orm::DatabaseBackend::Postgres => {
                r"
CREATE TABLE IF NOT EXISTS scan_records (
    trace_id UUID PRIMARY KEY NOT NULL,
    tenant_id UUID NOT NULL,
    connector_id VARCHAR(255) NOT NULL,
    content_kind VARCHAR(32) NOT NULL,
    action VARCHAR(16) NOT NULL,
    findings JSONB NOT NULL,
    redacted_payload TEXT,
    created_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_scan_records_tenant_id ON scan_records(tenant_id);
CREATE INDEX IF NOT EXISTS idx_scan_records_created_at ON scan_records(created_at);
                "
            }
            sea_orm::DatabaseBackend::MySql => {
                r"
CREATE TABLE IF NOT EXISTS scan_records (
    trace_id VARCHAR(36) PRIMARY KEY NOT NULL,
    tenant_id VARCHAR(36) NOT NULL,
    connector_id VARCHAR(255) NOT NULL,
    content_kind VARCHAR(32) NOT NULL,
    action VARCHAR(16) NOT NULL,
    findings JSON NOT NULL,
    redacted_payload TEXT,
    created_at TIMESTAMP NOT NULL,
    INDEX idx_scan_records_tenant_id (tenant_id),
    INDEX idx_scan_records_created_at (created_at)
);
                "
            }
            sea_orm::DatabaseBackend::Sqlite => {
                r"
CREATE TABLE IF NOT EXISTS scan_records (
    trace_id TEXT PRIMARY KEY NOT NULL,
    tenant_id TEXT NOT NULL,
    connector_id TEXT NOT NULL,
    content_kind TEXT NOT NULL,
    action TEXT NOT NULL,
    findings TEXT NOT NULL,
    redacted_payload TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_scan_records_tenant_id ON scan_records(tenant_id);
CREATE INDEX IF NOT EXISTS idx_scan_records_created_at ON scan_records(created_at);
                "
            }
            other => {
                return Err(DbErr::Migration(format!(
                    "unsupported database backend: {other:?}"
                )));
            }
        };

        conn.execute_unprepared(sql).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("DROP TABLE IF EXISTS scan_records;")
            .await?;
        Ok(())
    }
}
