//! prompt-scanner gear.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use prompt_scanner_sdk::PromptScannerClientV1;
use sea_orm_migration::MigrationTrait;
use tokio_util::sync::CancellationToken;
use toolkit::api::OpenApiRegistry;
use toolkit::lifecycle::ReadySignal;
use toolkit::{DatabaseCapability, Gear, GearCtx, RestApiCapability};
use toolkit_db::DBProvider;
use toolkit_db::odata::LimitCfg;
use tracing::info;

use authz_resolver_sdk::{AuthZResolverApi, PolicyEnforcer};

use crate::api::rest::routes;
use crate::config::{FallbackPolicy, PromptScannerConfig};
use crate::domain::local_client::PromptScannerLocalClient;
use crate::domain::policy_provider::ConfigPolicyProvider;
use crate::domain::service::Service;
use crate::infra::storage::OrmScanRecordRepository;

/// Concrete `Service` type used with `SeaORM` repositories.
///
/// Lives in the composition root (`gear.rs`) so the domain layer stays
/// generic over the repository traits, per the DDD-light layering rules.
pub(crate) type ConcreteService = Service<OrmScanRecordRepository>;

#[toolkit::gear(
    name = "prompt-scanner",
    deps = [authz_resolver, types_registry, file_parser],
    capabilities = [db, rest, stateful],
    lifecycle(entry = "serve", stop_timeout = "10s", await_ready)
)]
pub struct PromptScanner {
    service: OnceLock<Arc<ConcreteService>>,
    job_poll_interval: OnceLock<Duration>,
}

impl Default for PromptScanner {
    fn default() -> Self {
        Self {
            service: OnceLock::new(),
            job_poll_interval: OnceLock::new(),
        }
    }
}

#[async_trait]
impl Gear for PromptScanner {
    async fn init(&self, ctx: &GearCtx) -> anyhow::Result<()> {
        let cfg: PromptScannerConfig = ctx.config_or_default()?;
        info!(
            default_page_size = cfg.default_page_size,
            max_page_size = cfg.max_page_size,
            backend_timeout_ms = cfg.backend_timeout_ms,
            "Loaded prompt_scanner config"
        );

        let db: Arc<DBProvider<toolkit_db::DbError>> = Arc::new(ctx.db_required()?);

        let authz = ctx
            .client_hub()
            .get::<dyn AuthZResolverApi>()
            .map_err(|e| anyhow::anyhow!("failed to get AuthZ resolver: {e}"))?;
        let policy_enforcer = PolicyEnforcer::new(authz);

        let limit_cfg = LimitCfg {
            default: u64::from(cfg.default_page_size),
            max: u64::from(cfg.max_page_size),
        };
        let scan_repo = Arc::new(OrmScanRecordRepository::new(limit_cfg));
        let fallback_policy = cfg.fallback_policy.map(FallbackPolicy::into_template);
        if let Some(policy) = &fallback_policy {
            ConcreteService::validate_policy(policy)
                .map_err(|e| anyhow::anyhow!("invalid fallback_policy: {e}"))?;
        }
        let default_policy_provider: Arc<dyn prompt_scanner_sdk::PolicyProvider> =
            Arc::new(ConfigPolicyProvider::new(fallback_policy));

        let service = Arc::new(Service::new(
            db,
            ctx.client_hub(),
            scan_repo,
            default_policy_provider,
            policy_enforcer,
            Duration::from_millis(cfg.backend_timeout_ms),
        ));

        self.service
            .set(service.clone())
            .map_err(|_| anyhow::anyhow!("{} gear already initialized", Self::MODULE_NAME))?;
        self.job_poll_interval
            .set(Duration::from_millis(cfg.job_poll_interval_ms))
            .map_err(|_| anyhow::anyhow!("{} gear already initialized", Self::MODULE_NAME))?;

        let api: Arc<dyn PromptScannerClientV1> = Arc::new(PromptScannerLocalClient::new(service));
        ctx.client_hub().register::<dyn PromptScannerClientV1>(api);

        Ok(())
    }
}

impl DatabaseCapability for PromptScanner {
    fn migrations(&self) -> Vec<Box<dyn MigrationTrait>> {
        use sea_orm_migration::MigratorTrait;
        info!("Providing prompt_scanner database migrations");
        crate::infra::storage::migrations::Migrator::migrations()
    }
}

impl RestApiCapability for PromptScanner {
    fn register_rest(
        &self,
        _ctx: &GearCtx,
        router: axum::Router,
        openapi: &dyn OpenApiRegistry,
    ) -> anyhow::Result<axum::Router> {
        info!("Registering prompt_scanner REST routes");

        let service = self
            .service
            .get()
            .ok_or_else(|| anyhow::anyhow!("Service not initialized"))?
            .clone();

        let router = routes::register_routes(router, openapi, service);

        info!("prompt_scanner REST routes registered successfully");
        Ok(router)
    }
}

impl PromptScanner {
    /// Stateful background task: drains the in-memory `scan_async` job
    /// queue at a fixed interval until cancelled.
    ///
    /// Scaffold-level durability: the queue lives in `Service` (an
    /// in-memory `Mutex<VecDeque<...>>`), so queued-but-unprocessed jobs
    /// are lost on restart. See `domain::service::Service` docs for the
    /// deferred "durable queue" note.
    async fn serve(
        self: Arc<Self>,
        cancel: CancellationToken,
        ready: ReadySignal,
    ) -> anyhow::Result<()> {
        let service = self
            .service
            .get()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Service not initialized"))?;
        let poll_interval = self
            .job_poll_interval
            .get()
            .copied()
            .unwrap_or(Duration::from_millis(200));

        ready.notify();

        let mut interval = tokio::time::interval(poll_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    info!("prompt_scanner job worker shutting down");
                    break;
                }
                _ = interval.tick() => {
                    // Drain what's queued right now; leave the rest for the
                    // next tick rather than looping unboundedly, so a burst
                    // of jobs can't starve the cancellation check.
                    while service.process_next_job().await {
                        if cancel.is_cancelled() {
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
