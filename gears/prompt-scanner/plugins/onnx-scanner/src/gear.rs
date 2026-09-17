//! ONNX-model-backed PII scanner backend plugin gear.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use onnx_runner_sdk::OnnxRunnerClientV1;
use prompt_scanner_sdk::{ScannerBackendClient, ScannerBackendSpecV1};
use toolkit::Gear;
use toolkit::client_hub::ClientScope;
use toolkit::context::GearCtx;
use toolkit::gts::PluginV1;
use tracing::info;
use types_registry_sdk::{RegisterResult, TypesRegistryClient};

use crate::config::{OnnxScannerConfig, PiiBackendConfig};
use crate::domain::Service;
use crate::domain::backend::PiiBackend;
use crate::domain::bio_backend::BioTokenClassifierBackend;
use crate::domain::gliner2::Gliner2Backend;

/// `onnx-runner`-backed PII detection `ScannerBackend` plugin. Replaces
/// `pii-plugin` (retired) — see `gears/onnx-runner/docs/DESIGN.md`.
/// Prompt-injection detection is out of scope for now; the Python
/// `GLiNER2`-sidecar-backed `prompt-injection-plugin` was removed entirely
/// rather than kept as an external dependency.
///
/// **Plugin registration pattern:**
/// - The `prompt-scanner` gear owns the `ScannerBackendSpecV1` GTS schema
///   (published automatically via link-time inventory — no per-init
///   registration needed on the gateway side).
/// - This plugin registers its instance (implementation metadata) and its
///   scoped client (`ClientHub` entry) so `prompt-scanner`'s fan-out
///   resolution logic can discover and call it.
#[toolkit::gear(name = "onnx-scanner", deps = [types_registry, onnx_runner])]
pub struct OnnxScanner {
    service: OnceLock<Arc<Service>>,
}

impl Default for OnnxScanner {
    fn default() -> Self {
        Self {
            service: OnceLock::new(),
        }
    }
}

#[async_trait]
impl Gear for OnnxScanner {
    async fn init(&self, ctx: &GearCtx) -> anyhow::Result<()> {
        if self.service.get().is_some() {
            anyhow::bail!("{} gear already initialized", Self::MODULE_NAME);
        }

        let cfg: OnnxScannerConfig = ctx.config_or_default()?;
        info!(
            vendor = %cfg.vendor,
            priority = cfg.priority,
            backend = ?cfg.backend,
            "Loaded plugin configuration"
        );

        let onnx_client = ctx.client_hub().get::<dyn OnnxRunnerClientV1>()?;
        let backend: Arc<dyn PiiBackend> = match cfg.backend.clone() {
            PiiBackendConfig::BioTokenClassifier { model_id, labels } => Arc::new(
                BioTokenClassifierBackend::new(onnx_client, model_id, labels),
            ),
            PiiBackendConfig::Gliner2 {
                model_id,
                labels,
                decode_threshold,
            } => {
                let backend = Gliner2Backend::load(onnx_client, model_id, labels, decode_threshold)
                    .await
                    .map_err(|e| {
                        anyhow::anyhow!("onnx-scanner: failed to load gliner2 backend: {e}")
                    })?;
                info!(
                    prefix_tokens = backend.prefix_tokens(),
                    "onnx-scanner: gliner2 schema prefix built"
                );
                Arc::new(backend)
            }
        };

        let service = Arc::new(Service::new(
            backend,
            cfg.category_prefix.clone(),
            cfg.defer_threshold_words,
            cfg.redact_threshold,
            cfg.warn_threshold,
        ));

        let (instance_id, instance_json) = PluginV1::<ScannerBackendSpecV1>::build_registration(
            "cf.builtin.onnx_pii.detector.v1",
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

        info!(instance_id = %instance_id, "onnx-scanner registered");
        Ok(())
    }
}
