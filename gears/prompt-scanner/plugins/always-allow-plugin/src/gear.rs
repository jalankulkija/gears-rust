//! Always-allow scanner backend plugin gear.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use prompt_scanner_sdk::{ScannerBackendClient, ScannerBackendSpecV1};
use toolkit::Gear;
use toolkit::client_hub::ClientScope;
use toolkit::context::GearCtx;
use toolkit::gts::PluginV1;
use tracing::info;
use types_registry_sdk::{RegisterResult, TypesRegistryClient};

use crate::config::AlwaysAllowPluginConfig;
use crate::domain::Service;

/// Minimal "always allow" scanner backend plugin.
///
/// **Plugin registration pattern:**
/// - The `prompt-scanner` gear owns the `ScannerBackendSpecV1` GTS schema
///   (published automatically via link-time inventory — no per-init
///   registration needed on the gateway side).
/// - This plugin registers its instance (implementation metadata) and its
///   scoped client (`ClientHub` entry) so `prompt-scanner`'s fan-out
///   resolution logic can discover and call it.
#[toolkit::gear(name = "always-allow-plugin", deps = [types_registry])]
pub struct AlwaysAllowPlugin {
    service: OnceLock<Arc<Service>>,
}

impl Default for AlwaysAllowPlugin {
    fn default() -> Self {
        Self {
            service: OnceLock::new(),
        }
    }
}

#[async_trait]
impl Gear for AlwaysAllowPlugin {
    async fn init(&self, ctx: &GearCtx) -> anyhow::Result<()> {
        if self.service.get().is_some() {
            anyhow::bail!("{} gear already initialized", Self::MODULE_NAME);
        }

        let cfg: AlwaysAllowPluginConfig = ctx.config_or_default()?;
        info!(vendor = %cfg.vendor, priority = cfg.priority, "Loaded plugin configuration");

        let service = Arc::new(Service);

        let (instance_id, instance_json) = PluginV1::<ScannerBackendSpecV1>::build_registration(
            "cf.builtin.always_allow.detector.v1",
            cfg.vendor.clone(),
            cfg.priority,
        )?;

        let registry = ctx.client_hub().get::<dyn TypesRegistryClient>()?;
        let results = registry.register(vec![instance_json]).await?;
        RegisterResult::ensure_all_ok(&results)?;

        self.service
            .set(service.clone())
            .map_err(|_| anyhow::anyhow!("{} gear already initialized", Self::MODULE_NAME))?;

        let api: Arc<dyn ScannerBackendClient> = service;
        ctx.client_hub()
            .register_scoped::<dyn ScannerBackendClient>(ClientScope::gts_id(&instance_id), api);

        info!(instance_id = %instance_id, "always-allow-plugin registered");
        Ok(())
    }
}
