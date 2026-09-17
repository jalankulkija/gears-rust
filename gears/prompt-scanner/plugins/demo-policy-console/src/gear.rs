//! `demo-policy-console` gear.
//!
//! Temporary demo control panel for `prompt-scanner`'s PII detection — see
//! `docs/POLICY_PROVIDER_AND_DEMO_CONSOLE.md`. Registers itself as a
//! `PolicyProviderSpecV1` plugin instance (so `prompt-scanner` resolves it
//! via `Service::resolve_policy_provider` instead of its own DB-backed
//! default) and serves its own REST API + a small web UI for toggling PII
//! detection on/off and choosing the response action.
//!
//! **Plugin registration pattern** (mirrors `regex-scanner`'s
//! `ScannerBackendSpecV1` registration, see that gear's docs):
//! - `prompt-scanner` owns the `PolicyProviderSpecV1` GTS schema.
//! - This plugin registers its instance (implementation metadata) and its
//!   scoped client (`ClientHub` entry) so `prompt-scanner`'s single-instance
//!   policy provider resolution can discover and call it.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use prompt_scanner_sdk::{PolicyProvider, PolicyProviderSpecV1};
use toolkit::Gear;
use toolkit::client_hub::ClientScope;
use toolkit::context::GearCtx;
use toolkit::gts::PluginV1;
use tracing::info;
use types_registry_sdk::{RegisterResult, TypesRegistryClient};

use crate::api::rest::routes;
use crate::config::DemoPolicyConsoleConfig;
use crate::domain::{DemoPolicyState, Service};

#[toolkit::gear(
    name = "demo-policy-console",
    deps = [types_registry, prompt_scanner],
    capabilities = [rest]
)]
pub struct DemoPolicyConsole {
    service: OnceLock<Arc<Service>>,
}

impl Default for DemoPolicyConsole {
    fn default() -> Self {
        Self {
            service: OnceLock::new(),
        }
    }
}

#[async_trait]
impl Gear for DemoPolicyConsole {
    async fn init(&self, ctx: &GearCtx) -> anyhow::Result<()> {
        if self.service.get().is_some() {
            anyhow::bail!("{} gear already initialized", Self::MODULE_NAME);
        }

        let cfg: DemoPolicyConsoleConfig = ctx.config_or_default()?;
        info!(
            vendor = %cfg.vendor,
            priority = cfg.priority,
            pii_enabled = cfg.pii_enabled,
            pii_action = ?cfg.pii_action,
            "Loaded demo-policy-console configuration"
        );

        let initial_state = DemoPolicyState {
            pii_enabled: cfg.pii_enabled,
            pii_action: cfg.pii_action,
            enabled_backends: cfg.enabled_backends.clone(),
        };
        let service = Arc::new(Service::new(initial_state, ctx.client_hub()));

        let (instance_id, instance_json) = PluginV1::<PolicyProviderSpecV1>::build_registration(
            "cf.demo.policy_console.provider.v1",
            cfg.vendor.clone(),
            cfg.priority,
        )?;

        let registry = ctx.client_hub().get::<dyn TypesRegistryClient>()?;
        let results = registry.register(vec![instance_json]).await?;
        RegisterResult::ensure_all_ok(&results)?;

        self.service
            .set(service.clone())
            .map_err(|_| anyhow::anyhow!("{} gear already initialized", Self::MODULE_NAME))?;

        let provider: Arc<dyn PolicyProvider> = service;
        ctx.client_hub()
            .register_scoped::<dyn PolicyProvider>(ClientScope::gts_id(&instance_id), provider);

        info!(instance_id = %instance_id, "demo-policy-console registered as prompt-scanner's PolicyProvider");
        Ok(())
    }
}

impl toolkit::RestApiCapability for DemoPolicyConsole {
    fn register_rest(
        &self,
        _ctx: &GearCtx,
        router: axum::Router,
        openapi: &dyn toolkit::api::OpenApiRegistry,
    ) -> anyhow::Result<axum::Router> {
        let service = self
            .service
            .get()
            .ok_or_else(|| anyhow::anyhow!("Service not initialized"))?
            .clone();

        info!("Registering demo-policy-console REST routes");
        Ok(routes::register_routes(router, openapi, service))
    }
}
