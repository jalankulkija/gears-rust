//! Deterministic regex/validator scanner backend plugin gear.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use prompt_scanner_sdk::{ScannerBackendClient, ScannerBackendSpecV1};
use toolkit::Gear;
use toolkit::client_hub::ClientScope;
use toolkit::context::GearCtx;
use toolkit::gts::PluginV1;
use tracing::{info, warn};
use types_registry_sdk::{RegisterResult, TypesRegistryClient};

use crate::config::RegexScannerConfig;
use crate::domain::{PatternSet, Service};

/// Deterministic (regex + validator) `ScannerBackend` plugin — no ONNX, no
/// `onnx-runner` dependency. See `gears/onnx-runner/docs/DESIGN.md`.
///
/// **Plugin registration pattern:**
/// - The `prompt-scanner` gear owns the `ScannerBackendSpecV1` GTS schema
///   (published automatically via link-time inventory — no per-init
///   registration needed on the gateway side).
/// - This plugin registers its instance (implementation metadata) and its
///   scoped client (`ClientHub` entry) so `prompt-scanner`'s fan-out
///   resolution logic can discover and call it.
#[toolkit::gear(name = "regex-scanner", deps = [types_registry])]
pub struct RegexScanner {
    service: OnceLock<Arc<Service>>,
}

impl Default for RegexScanner {
    fn default() -> Self {
        Self {
            service: OnceLock::new(),
        }
    }
}

#[async_trait]
impl Gear for RegexScanner {
    async fn init(&self, ctx: &GearCtx) -> anyhow::Result<()> {
        if self.service.get().is_some() {
            anyhow::bail!("{} gear already initialized", Self::MODULE_NAME);
        }

        let cfg: RegexScannerConfig = ctx.config_or_default()?;

        // Compile here, not on the request path: an invalid pattern must fail
        // startup loudly rather than degrade every scan afterwards.
        let patterns = Arc::new(PatternSet::compile(&cfg.patterns, cfg.entropy_threshold)?);
        if patterns.is_empty() {
            warn!(
                vendor = %cfg.vendor,
                "regex-scanner configured with an empty pattern set; this backend will \
                 report no findings"
            );
        }
        info!(
            vendor = %cfg.vendor,
            priority = cfg.priority,
            patterns = patterns.len(),
            categories = patterns.categories().len(),
            entropy_threshold = cfg.entropy_threshold,
            "Loaded plugin configuration"
        );

        let service = Arc::new(Service::new(patterns));

        let (instance_id, instance_json) = PluginV1::<ScannerBackendSpecV1>::build_registration(
            "cf.builtin.regex_scanner.detector.v1",
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

        info!(instance_id = %instance_id, "regex-scanner registered");
        Ok(())
    }
}
