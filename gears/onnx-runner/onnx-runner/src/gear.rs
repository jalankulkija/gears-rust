//! Generic ONNX execution gear. See `docs/DESIGN.md` for the full design.
//!
//! **This gear exposes no HTTP API of its own, by design** — see
//! `cpt-cf-onnx-runner-constraint-no-http-surface` in `docs/DESIGN.md` §3.5.
//! It is an in-process compute primitive reached through `ClientHub`, in the
//! same category as `authn-resolver`, `authz-resolver`, `tenant-resolver`,
//! and `cluster`, none of which have a REST surface either. It owns no
//! tenant-facing resource, so there is nothing for an endpoint to address.
//! Operator-visible readiness belongs in the platform `Healthcheck` registry
//! (`toolkit::healthcheck`), which API Gateway aggregates into `/health` and
//! `/readyz`. If a genuine out-of-process consumer ever appears, the answer
//! is the OoP/gRPC pattern, not tensors over HTTP.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use onnx_runner_sdk::{
    ModelFailureReason, ModelInfo, ModelState, NamedTensor, OnnxRunnerClientV1, OnnxRunnerError,
    TokenEncoding,
};
use toolkit::contracts::OpenApiRegistry;
use toolkit::{Gear, GearCtx, Healthcheck, RestApiCapability};
use toolkit_security::SecurityContext;
use tracing::{error, info, warn};

use crate::config::{ModelConfig, OnnxRunnerConfig};
use crate::domain::metrics::{LoadOutcome, OnnxRunnerMetricsPort};
use crate::infra::metrics::OnnxRunnerMetricsMeter;
use crate::infra::ort_session::{LoadFailure, LoadedModel};
use crate::infra::tokenizer::LoadedTokenizer;

/// One configured model: either a loaded session pool with its tokenizer, or
/// the classified reason it is not usable.
///
/// Both cases are kept, which is the whole point of this type — a model that
/// failed to load is *unavailable*, not absent, and a consumer must be able to
/// tell that from a typo'd id.
///
/// `#[doc(hidden)] pub` for the same reason as [`load_models`] — it appears in
/// that function's return type, which an integration test has to name. Not
/// public API.
#[doc(hidden)]
pub enum ModelEntry {
    Ready(Box<LoadedEntry>),
    Unavailable(ModelFailureReason),
}

impl ModelEntry {
    fn state(&self) -> ModelState {
        match self {
            Self::Ready(_) => ModelState::Ready,
            Self::Unavailable(reason) => ModelState::Unavailable { reason: *reason },
        }
    }
}

/// A loaded model's session pool plus its tokenizer.
#[doc(hidden)]
pub struct LoadedEntry {
    model: LoadedModel,
    tokenizer: LoadedTokenizer,
}

/// Why model loading failed, with the model that failed named.
///
/// Only reachable when `require_all_models` is set: with the permissive
/// default a per-model failure becomes [`ModelEntry::Unavailable`] instead.
#[derive(Debug, thiserror::Error)]
#[error("model {model_id}: {source}")]
pub struct ModelLoadError {
    pub model_id: String,
    #[source]
    pub source: OnnxRunnerError,
}

/// Why this gear failed to start. See [`ort_bootstrap`] for the two variants
/// that oblige the caller to terminate the process.
pub type ModelInitError = ort_bootstrap::BootstrapError<ModelLoadError>;

/// Loads every configured model and tokenizer, off the async runtime and under
/// a bounded wait.
///
/// A model that fails to load is recorded [`ModelEntry::Unavailable`] and the
/// gear still starts — one bad artifact must not take down every other gear in
/// the process. `require_all_models` restores fail-fast for deployments that
/// prefer it.
///
/// Bounded because `ort` 2.0.0-rc.12 hangs instead of erroring when the ONNX
/// Runtime it `dlopen`s via `ORT_DYLIB_PATH` cannot be loaded — the mechanism,
/// and why the wedged thread must be abandoned rather than joined, is
/// documented once in [`ort_bootstrap`]. Provision a compatible runtime with
/// `scripts/fetch-onnxruntime.sh`.
///
/// Running off the runtime is right for a second reason too: loading reads and
/// hashes the whole model file and builds one session per replica, which is
/// exactly the blocking work that must never sit on an async worker.
///
/// # Visibility
///
/// `#[doc(hidden)] pub` only so `tests/model_load_timeout_tests.rs` can drive
/// this exact path; an integration test cannot reach a private item. Not
/// public API.
///
/// # Errors
///
/// See [`ModelInitError`] — two variants oblige the caller to terminate.
#[doc(hidden)]
pub async fn load_models(
    models: Vec<ModelConfig>,
    require_all_models: bool,
    metrics: Arc<dyn OnnxRunnerMetricsPort>,
    cancellation_token: &tokio_util::sync::CancellationToken,
) -> Result<BTreeMap<String, ModelEntry>, ModelInitError> {
    /// Per-session budget. Scaled by the number of sessions rather than flat,
    /// because a healthy load is dominated by real work — hashing a
    /// several-hundred-megabyte file and building one session per replica —
    /// and an unoptimized build makes that slower still. The hang never
    /// completes, so the only cost of a generous budget is how long a genuinely
    /// wedged startup takes to report itself.
    const PER_SESSION_LOAD_TIMEOUT: Duration = Duration::from_mins(1);

    let sessions: u32 = models
        .iter()
        .map(|m| u32::try_from(m.replicas).unwrap_or(u32::MAX))
        .fold(0_u32, u32::saturating_add)
        .max(1);

    ort_bootstrap::Bootstrap::new(
        "onnx-runner models",
        PER_SESSION_LOAD_TIMEOUT.saturating_mul(sessions),
    )
    .run(
        move || load_models_blocking(&models, require_all_models, &metrics),
        cancellation_token,
    )
    .await
}

/// The blocking half of [`load_models`]. Runs on an abandonable OS thread.
fn load_models_blocking(
    models: &[ModelConfig],
    require_all_models: bool,
    metrics: &Arc<dyn OnnxRunnerMetricsPort>,
) -> Result<BTreeMap<String, ModelEntry>, ModelLoadError> {
    let mut loaded = BTreeMap::new();

    for model_cfg in models {
        let started = Instant::now();
        let entry = match load_one(model_cfg, metrics) {
            Ok(entry) => {
                // `replicas × file size` — a floor, not a measurement: it
                // excludes ONNX Runtime's arenas and per-session thread
                // stacks. Enough to make an oversized config visible in the
                // startup log without reaching for a profiler.
                let resident = resident_bytes_estimate(model_cfg);
                metrics.record_model_load(
                    &model_cfg.model_id,
                    LoadOutcome::Ok,
                    started.elapsed(),
                    resident,
                );
                info!(
                    model_id = %model_cfg.model_id,
                    replicas = model_cfg.replicas,
                    resident_bytes = resident,
                    elapsed_ms = started.elapsed().as_millis(),
                    "onnx-runner: loaded model"
                );
                ModelEntry::Ready(Box::new(entry))
            }
            Err(failure) if require_all_models => {
                metrics.record_model_load(
                    &model_cfg.model_id,
                    LoadOutcome::Failed(failure.reason),
                    started.elapsed(),
                    0,
                );
                return Err(ModelLoadError {
                    model_id: model_cfg.model_id.clone(),
                    source: failure.source,
                });
            }
            Err(failure) => {
                metrics.record_model_load(
                    &model_cfg.model_id,
                    LoadOutcome::Failed(failure.reason),
                    started.elapsed(),
                    0,
                );
                degraded_entry(&model_cfg.model_id, &failure)
            }
        };
        loaded.insert(model_cfg.model_id.clone(), entry);
    }

    if !models.is_empty() && !loaded.values().any(|e| e.state().is_ready()) {
        // Not a startup failure under the permissive default — but a gear
        // serving nothing is almost never what the operator intended, and it
        // should not be something they discover from a consumer's coverage
        // report.
        error!(
            configured = models.len(),
            "onnx-runner: no model loaded successfully; every inference request will fail"
        );
    }

    Ok(loaded)
}

/// Logs a call rejected before it reached a session.
///
/// At `debug`, not `warn`: an unknown model id or an oversized input is a
/// caller-side mistake, and a misbehaving consumer must not be able to fill an
/// operator's logs. The metrics counters are where the *rate* of these belongs.
///
/// Only the error is logged. It carries model ids, tensor names and sizes —
/// never payload content — which is what
/// `cpt-cf-onnx-runner-principle-no-payload-in-telemetry` requires and what
/// `tests/telemetry_sentinel_tests.rs` enforces.
fn log_dispatch_failure(error: &OnnxRunnerError) {
    tracing::debug!(%error, "onnx-runner: call rejected before execution");
}

/// Records a model as unavailable, logging the failure once — here, where
/// the classification is known.
///
/// `reason` is the stable code a consumer branches on; `error` carries the
/// detail an operator needs and never leaves this process.
fn degraded_entry(model_id: &str, failure: &LoadFailure) -> ModelEntry {
    warn!(
        model_id = %model_id,
        reason = failure.reason.as_code(),
        error = %failure.source,
        "onnx-runner: model unavailable; the gear will start without it"
    );
    ModelEntry::Unavailable(failure.reason)
}

/// Loads one model and its tokenizer, classifying whichever step fails.
fn load_one(
    cfg: &ModelConfig,
    metrics: &Arc<dyn OnnxRunnerMetricsPort>,
) -> Result<LoadedEntry, LoadFailure> {
    let model = LoadedModel::load(cfg)?.with_metrics(Arc::clone(metrics));
    let tokenizer_path = cfg.model_dir.join(&cfg.tokenizer_file);
    let tokenizer = LoadedTokenizer::load(&tokenizer_path)
        .map_err(|source| LoadFailure {
            reason: ModelFailureReason::TokenizerInvalid,
            source,
        })?
        .with_metrics(Arc::clone(metrics));
    Ok(LoadedEntry { model, tokenizer })
}

/// Estimated resident footprint for a loaded model.
///
/// `replicas × model file size`. A floor rather than a measurement — ONNX
/// Runtime's arenas, per-session thread stacks and pre-packed weights are not
/// counted — but it is the number that makes "4 replicas of a 446 MB model"
/// visible at startup instead of in an out-of-memory kill.
fn resident_bytes_estimate(cfg: &ModelConfig) -> u64 {
    let file_size = std::fs::metadata(cfg.model_dir.join(&cfg.model_file)).map_or(0, |m| m.len());
    file_size.saturating_mul(u64::try_from(cfg.replicas).unwrap_or(u64::MAX))
}

/// In-process implementation of [`OnnxRunnerClientV1`], registered in
/// `ClientHub` by [`OnnxRunner::init`].
pub struct OnnxRunnerLocalClient {
    /// Every *configured* model, not only the ones that loaded.
    models: BTreeMap<String, ModelEntry>,
}

impl OnnxRunnerLocalClient {
    /// Builds the client over an already-loaded model map.
    ///
    /// `#[doc(hidden)] pub` only so `tests/model_lifecycle_tests.rs` can drive
    /// the real `list_models` projection rather than a reimplementation of it.
    /// Not public API — consumers resolve this through `ClientHub`.
    #[doc(hidden)]
    #[must_use]
    pub const fn new(models: BTreeMap<String, ModelEntry>) -> Self {
        Self { models }
    }

    /// Rejects oversized text before tokenizing it.
    ///
    /// Rejection rather than truncation is deliberate: silently shortening a
    /// caller's text would make the offsets it gets back describe a document
    /// that was never scanned. A truncation *policy* belongs to the tokenizer
    /// feature that owns padding and windowing, not to a size guard.
    fn check_text(entry: &LoadedEntry, model_id: &str, text: &str) -> Result<(), OnnxRunnerError> {
        let limit = entry.model.limits().text_bytes;
        if text.len() > limit {
            return Err(OnnxRunnerError::InvalidInput {
                model_id: model_id.to_owned(),
                tensor: "<text>".to_owned(),
                message: format!("{} bytes of text, over the limit of {limit}", text.len()),
            });
        }
        Ok(())
    }

    /// Resolves a model id to a usable entry, keeping the two failure modes
    /// apart: not configured at all (a caller mistake) versus configured but
    /// unusable (an outage the consumer must report as degraded coverage).
    fn ready_entry(&self, model_id: &str) -> Result<&LoadedEntry, OnnxRunnerError> {
        match self.models.get(model_id) {
            Some(ModelEntry::Ready(entry)) => Ok(entry),
            Some(ModelEntry::Unavailable(reason)) => Err(OnnxRunnerError::ModelUnavailable {
                model_id: model_id.to_owned(),
                reason: *reason,
            }),
            None => Err(OnnxRunnerError::UnknownModel {
                model_id: model_id.to_owned(),
            }),
        }
    }
}

/// Note the `_ctx` bindings throughout: this gear is a trusted in-process
/// compute primitive and enforces no authorization, deliberately. The full
/// reasoning is on [`OnnxRunnerClientV1`] and in ADR 0011; the short version
/// is that the gear owns no tenant-scoped resource, so there is nothing for an
/// authorization decision to be about. Do not read the unused parameter as an
/// oversight to be fixed by adding a check here — enforcement needs a policy
/// source this gear does not have.
#[async_trait]
impl OnnxRunnerClientV1 for OnnxRunnerLocalClient {
    #[tracing::instrument(
        name = "onnx_runner.client.run",
        skip_all,
        fields(model_id = %model_id, tensors = inputs.len())
    )]
    async fn run(
        &self,
        _ctx: &SecurityContext,
        model_id: &str,
        inputs: Vec<NamedTensor>,
    ) -> Result<Vec<NamedTensor>, OnnxRunnerError> {
        // The span exists at this layer, not only in `infra`, because a call
        // rejected here — unknown model, unavailable model — never reaches a
        // session and would otherwise leave no trace at all.
        self.ready_entry(model_id)
            .inspect_err(log_dispatch_failure)?
            .model
            .run(inputs)
            .await
    }

    #[tracing::instrument(
        name = "onnx_runner.client.encode",
        skip_all,
        fields(model_id = %model_id, text_bytes = text.len())
    )]
    async fn encode(
        &self,
        _ctx: &SecurityContext,
        model_id: &str,
        text: &str,
    ) -> Result<TokenEncoding, OnnxRunnerError> {
        let entry = self
            .ready_entry(model_id)
            .inspect_err(log_dispatch_failure)?;
        Self::check_text(entry, model_id, text).inspect_err(log_dispatch_failure)?;
        entry.tokenizer.encode(model_id, text.to_owned()).await
    }

    #[tracing::instrument(
        name = "onnx_runner.client.encode_fragment",
        skip_all,
        fields(model_id = %model_id, text_bytes = fragment.len())
    )]
    async fn encode_fragment(
        &self,
        _ctx: &SecurityContext,
        model_id: &str,
        fragment: &str,
    ) -> Result<Vec<i64>, OnnxRunnerError> {
        let entry = self
            .ready_entry(model_id)
            .inspect_err(log_dispatch_failure)?;
        Self::check_text(entry, model_id, fragment).inspect_err(log_dispatch_failure)?;
        entry
            .tokenizer
            .encode_fragment(model_id, fragment.to_owned())
            .await
    }

    /// Reports every configured model, including the ones that failed to load.
    /// A caller that finds no entry is looking at a typo; one that finds
    /// `Unavailable` is looking at an outage.
    async fn list_models(&self, _ctx: &SecurityContext) -> Result<Vec<ModelInfo>, OnnxRunnerError> {
        Ok(self.model_snapshot())
    }
}

impl OnnxRunnerLocalClient {
    /// Every configured model's current state, read now rather than at load.
    ///
    /// Shared by [`OnnxRunnerClientV1::list_models`] and the platform
    /// healthcheck so operators and consumers cannot be told different
    /// stories about the same model. Cheap by construction: it clones
    /// recorded metadata and touches no session.
    #[must_use]
    pub fn model_snapshot(&self) -> Vec<ModelInfo> {
        self.models
            .iter()
            .map(|(model_id, entry)| match entry {
                ModelEntry::Ready(loaded) => {
                    let mut info = loaded.model.info.clone();
                    // Both are read now rather than frozen at load: replicas
                    // are lost to panicking inferences and rebuilt on the next
                    // call, and a model that loses all of them becomes
                    // unavailable at runtime.
                    info.state = loaded.model.state();
                    if let Some(loaded_info) = info.loaded.as_mut() {
                        loaded_info.replicas = loaded.model.live_replicas();
                    }
                    info
                }
                ModelEntry::Unavailable(reason) => ModelInfo {
                    model_id: model_id.clone(),
                    state: ModelState::Unavailable { reason: *reason },
                    loaded: None,
                },
            })
            .collect()
    }
}

// `rest` is declared for the `healthcheck` hook alone. See the
// `RestApiCapability` impl below: `register_rest` adds no routes, and a test
// enforces that it never starts to.
#[toolkit::gear(name = "onnx-runner", capabilities = [rest])]
pub struct OnnxRunner {
    client: OnceLock<Arc<OnnxRunnerLocalClient>>,
}

impl Default for OnnxRunner {
    fn default() -> Self {
        Self {
            client: OnceLock::new(),
        }
    }
}

#[async_trait]
impl Gear for OnnxRunner {
    async fn init(&self, ctx: &GearCtx) -> anyhow::Result<()> {
        let cfg: OnnxRunnerConfig = ctx.config_or_default()?;

        // Meter obtained from the platform provider rather than a global
        // reference held here, mirroring `file-storage`'s wiring.
        let metrics_scope =
            opentelemetry::InstrumentationScope::builder(Self::MODULE_NAME.to_owned()).build();
        let metrics: Arc<dyn OnnxRunnerMetricsPort> = Arc::new(OnnxRunnerMetricsMeter::new(
            &opentelemetry::global::meter_with_scope(metrics_scope),
            "onnx_runner",
        ));

        let models = load_models(
            cfg.models,
            cfg.require_all_models,
            Arc::clone(&metrics),
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
                    "onnx-runner: model loading left an abandoned thread; the process must \
                     terminate"
                );
            }
            anyhow::anyhow!("onnx-runner: {e}")
        })?;

        let client = Arc::new(OnnxRunnerLocalClient::new(models));
        self.client
            .set(client.clone())
            .map_err(|_| anyhow::anyhow!("{} gear already initialized", Self::MODULE_NAME))?;

        let api: Arc<dyn OnnxRunnerClientV1> = client;
        ctx.client_hub().register::<dyn OnnxRunnerClientV1>(api);

        Ok(())
    }
}

/// **Not an HTTP surface.** This impl exists solely for the `healthcheck`
/// hook, which is how the platform learns whether this gear's models loaded
/// (`cpt-cf-onnx-runner-constraint-no-http-surface`,
/// [`crate::health`]). `register_rest` returns the router it was handed,
/// unchanged, and `registers_no_rest_routes` in
/// `tests/healthcheck_tests.rs` fails if that ever stops being true.
///
/// If you are here because you want to add a route: the answer for an
/// out-of-process consumer is the OoP/gRPC pattern, not tensors over HTTP.
impl RestApiCapability for OnnxRunner {
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
        let client = self.client.get()?;
        Some(Arc::new(crate::health::ModelReadiness::new(Arc::clone(
            client,
        ))))
    }
}
