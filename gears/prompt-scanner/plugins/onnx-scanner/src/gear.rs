//! ONNX-model-backed PII scanner backend plugin gear.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use prompt_scanner_sdk::{ScannerBackendClient, ScannerBackendSpecV1};
use toolkit::client_hub::ClientScope;
use toolkit::context::GearCtx;
use toolkit::contracts::OpenApiRegistry;
use toolkit::gts::PluginV1;
use toolkit::{Gear, Healthcheck, RestApiCapability};
use tracing::info;
use types_registry_sdk::{RegisterResult, TypesRegistryClient};

use crate::config::{OnnxScannerConfig, PiiBackendConfig};
use crate::domain::Service;
use crate::domain::backend::PiiBackend;
use crate::domain::bio_backend::BioTokenClassifierBackend;
use crate::domain::gliner2::Gliner2Backend;
use crate::runtime::health::ModelReadiness;
use crate::runtime::metrics_port::RuntimeMetricsPort;
use crate::runtime::{LocalRuntime, ModelRuntime, RuntimeMetricsMeter, load_models};

/// ONNX-model-backed PII detection `ScannerBackend` plugin. Replaces
/// `pii-plugin` (retired) and the standalone `onnx-runner` gear — it loads and
/// runs its models itself (see `crate::runtime`).
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
// `rest` is declared for the `healthcheck` hook alone: `register_rest` adds no
// routes (see the `RestApiCapability` impl below).
#[toolkit::gear(name = "onnx-scanner", deps = [types_registry], capabilities = [rest])]
pub struct OnnxScanner {
    service: OnceLock<Arc<Service>>,
    runtime: OnceLock<Arc<LocalRuntime>>,
}

impl Default for OnnxScanner {
    fn default() -> Self {
        Self {
            service: OnceLock::new(),
            runtime: OnceLock::new(),
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

        // Meter obtained from the platform provider rather than a global
        // reference held here.
        let metrics_scope =
            opentelemetry::InstrumentationScope::builder(Self::MODULE_NAME.to_owned()).build();
        let metrics: Arc<dyn RuntimeMetricsPort> = Arc::new(RuntimeMetricsMeter::new(
            &opentelemetry::global::meter_with_scope(metrics_scope),
            "onnx_scanner",
        ));

        let models = load_models(
            cfg.models.clone(),
            cfg.require_all_models,
            metrics,
            ctx.cancellation_token(),
        )
        .await
        .map_err(|e| {
            if e.leaked_init_thread() {
                // Surfaced because the process must now terminate for the
                // leak to stay bounded; the operator needs to know why a
                // clean exit may not be available.
                tracing::error!(
                    error = %e,
                    "onnx-scanner: model loading left an abandoned thread; the process must \
                     terminate"
                );
            }
            anyhow::anyhow!("onnx-scanner: {e}")
        })?;
        let runtime = Arc::new(LocalRuntime::new(models));
        self.runtime
            .set(Arc::clone(&runtime))
            .map_err(|_| anyhow::anyhow!("{} gear already initialized", Self::MODULE_NAME))?;
        let onnx_client: Arc<dyn ModelRuntime> = runtime;
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

/// **Not an HTTP surface.** This impl exists solely for the `healthcheck` hook,
/// which is how the platform learns whether the models loaded. `register_rest`
/// returns the router it was handed, unchanged.
impl RestApiCapability for OnnxScanner {
    fn register_rest(
        &self,
        _ctx: &GearCtx,
        router: axum::Router,
        _openapi: &dyn OpenApiRegistry,
    ) -> anyhow::Result<axum::Router> {
        Ok(router)
    }

    fn healthcheck(&self, _ctx: &GearCtx) -> Option<Arc<dyn Healthcheck>> {
        // `None` before `init` has run, which is honest: there is no model
        // state to report yet, and the platform treats an absent check as an
        // opt-out rather than as health.
        let runtime = self.runtime.get()?;
        Some(Arc::new(ModelReadiness::new(Arc::clone(runtime))))
    }
}
