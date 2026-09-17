//! OpenAI/ChatGPT traffic handler plugin gear.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use forward_proxy_sdk::{ProxyHandlerPluginClient, ProxyHandlerPluginSpecV1};
use prompt_scanner_sdk::PromptScannerClientV1;
use toolkit::Gear;
use toolkit::client_hub::ClientScope;
use toolkit::context::GearCtx;
use toolkit::gts::PluginV1;
use tracing::info;
use types_registry_sdk::{RegisterResult, TypesRegistryClient};

use crate::config::OpenAiInterceptorConfig;
use crate::domain::Service;

/// Handler plugin for `forward-proxy` that scans OpenAI/ChatGPT prompts and
/// inline attachments via `prompt-scanner`. See `gears/forward-proxy/docs/DESIGN.md`.
///
/// **Plugin registration pattern:** mirrors `prompt-scanner`'s own detector
/// plugins — this plugin registers its instance (implementation metadata)
/// and its scoped client (`ClientHub` entry) so `forward-proxy`'s dispatch
/// loop can discover and call it (see `infra::handler_registry` in the
/// `forward-proxy` crate).
#[toolkit::gear(name = "openai-interceptor-plugin", deps = [types_registry, prompt_scanner])]
pub struct OpenAiInterceptorPlugin {
    service: OnceLock<Arc<Service>>,
}

impl Default for OpenAiInterceptorPlugin {
    fn default() -> Self {
        Self {
            service: OnceLock::new(),
        }
    }
}

#[async_trait]
impl Gear for OpenAiInterceptorPlugin {
    async fn init(&self, ctx: &GearCtx) -> anyhow::Result<()> {
        if self.service.get().is_some() {
            anyhow::bail!("{} gear already initialized", Self::MODULE_NAME);
        }

        let cfg: OpenAiInterceptorConfig = ctx.config_or_default()?;
        info!(
            vendor = %cfg.vendor,
            priority = cfg.priority,
            tenant_id = %cfg.tenant_id,
            "Loaded openai-interceptor-plugin configuration"
        );

        let prompt_scanner = ctx
            .client_hub()
            .get::<dyn PromptScannerClientV1>()
            .map_err(|e| {
                anyhow::anyhow!(
                    "openai-interceptor-plugin: failed to get prompt-scanner client: {e}"
                )
            })?;

        let service = Arc::new(Service::new(
            prompt_scanner,
            cfg.tenant_id,
            cfg.connector_id.clone(),
            cfg.product_name.clone(),
        ));

        let (instance_id, instance_json) =
            PluginV1::<ProxyHandlerPluginSpecV1>::build_registration(
                "cf.builtin.openai_interceptor.handler.v1",
                cfg.vendor.clone(),
                cfg.priority,
            )?;

        let registry = ctx.client_hub().get::<dyn TypesRegistryClient>()?;
        let results = registry.register(vec![instance_json]).await?;
        RegisterResult::ensure_all_ok(&results)?;

        self.service
            .set(service.clone())
            .map_err(|_| anyhow::anyhow!("{} gear already initialized", Self::MODULE_NAME))?;

        let api: Arc<dyn ProxyHandlerPluginClient> = service;
        ctx.client_hub()
            .register_scoped::<dyn ProxyHandlerPluginClient>(
                ClientScope::gts_id(&instance_id),
                api,
            );

        info!(instance_id = %instance_id, "openai-interceptor-plugin registered");
        Ok(())
    }
}
