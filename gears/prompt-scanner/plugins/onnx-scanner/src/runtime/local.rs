//! The in-process model host: loads every configured model once, verifies it,
//! and serves tensor execution and tokenization to the detector backends.
//!
//! This is not a gear and exposes no HTTP API (see
//! `cpt-cf-onnx-runner-constraint-no-http-surface` in `docs/runtime/DESIGN.md`
//! §3.5). `onnx-scanner` owns it and builds it at `init`; readiness reaches
//! the platform through the `Healthcheck` registry ([`super::health`]).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use super::{
    ModelFailureReason, ModelInfo, ModelRuntime, ModelState, NamedTensor, RuntimeError,
    TokenEncoding,
};
use async_trait::async_trait;
use tracing::{error, info, warn};

use super::config::ModelConfig;
use super::metrics_port::{LoadOutcome, RuntimeMetricsPort};
use super::session::{LoadError, LoadFailure, LoadedModel};
use super::tokenizer::LoadedTokenizer;

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
    pub source: RuntimeError,
}

/// Why the model host failed to start.
#[derive(Debug, thiserror::Error)]
pub enum InitError {
    /// `require_all_models` is set and a model could not be loaded.
    #[error(transparent)]
    Model(#[from] ModelLoadError),
    /// ONNX Runtime did not load in time (a bad `ORT_DYLIB_PATH`). The thread
    /// that tried was abandoned, so the process must terminate: see
    /// [`InitError::leaked_init_thread`].
    #[error(
        "ONNX Runtime did not load within {seconds}s (check ORT_DYLIB_PATH); an init thread was \
         abandoned and the process must terminate"
    )]
    RuntimeHung { seconds: u64 },
    /// Shutdown was requested while models were loading.
    #[error("startup cancelled while models were loading")]
    Cancelled,
}

impl InitError {
    /// Whether a thread was left running that cannot be stopped. The caller
    /// must then terminate the process rather than retry.
    #[must_use]
    pub const fn leaked_init_thread(&self) -> bool {
        matches!(self, Self::RuntimeHung { .. })
    }
}

/// Loads every configured model and tokenizer.
///
/// A model that fails to load is recorded [`ModelEntry::Unavailable`] and the
/// host still starts — one bad artifact must not take down every other gear in
/// the process. `require_all_models` restores fail-fast for deployments that
/// prefer it.
///
/// The ONNX Runtime hang on an unloadable `ORT_DYLIB_PATH` is the toolkit's to
/// guard against (`toolkit_onnx_runtime::OnnxSession::open` opens under a
/// deadline on an abandonable thread); this function only ends the wait early
/// when shutdown is requested, so a wedged load never holds up graceful
/// shutdown for the full budget.
///
/// # Visibility
///
/// `#[doc(hidden)] pub` only so the integration tests can drive this exact
/// path. Not public API.
///
/// # Errors
///
/// See [`InitError`] — [`InitError::RuntimeHung`] obliges the caller to
/// terminate the process.
#[doc(hidden)]
pub async fn load_models(
    models: Vec<ModelConfig>,
    require_all_models: bool,
    metrics: Arc<dyn RuntimeMetricsPort>,
    cancellation_token: &tokio_util::sync::CancellationToken,
) -> Result<BTreeMap<String, ModelEntry>, InitError> {
    tokio::select! {
        loaded = load_models_inner(&models, require_all_models, &metrics) => loaded,
        () = cancellation_token.cancelled() => Err(InitError::Cancelled),
    }
}

async fn load_models_inner(
    models: &[ModelConfig],
    require_all_models: bool,
    metrics: &Arc<dyn RuntimeMetricsPort>,
) -> Result<BTreeMap<String, ModelEntry>, InitError> {
    let mut loaded = BTreeMap::new();

    for model_cfg in models {
        let started = Instant::now();
        let entry = match load_one(model_cfg, metrics).await {
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
                    "onnx-scanner: loaded model"
                );
                ModelEntry::Ready(Box::new(entry))
            }
            Err(LoadError::RuntimeHung { seconds }) => {
                return Err(InitError::RuntimeHung { seconds });
            }
            Err(LoadError::Failed(failure)) if require_all_models => {
                metrics.record_model_load(
                    &model_cfg.model_id,
                    LoadOutcome::Failed(failure.reason),
                    started.elapsed(),
                    0,
                );
                return Err(ModelLoadError {
                    model_id: model_cfg.model_id.clone(),
                    source: failure.source,
                }
                .into());
            }
            Err(LoadError::Failed(failure)) => {
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
        // Not a startup failure under the permissive default — but a host
        // serving nothing is almost never what the operator intended, and it
        // should not be something they discover from a consumer's coverage
        // report.
        error!(
            configured = models.len(),
            "onnx-scanner: no model loaded successfully; every inference request will fail"
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
fn log_dispatch_failure(error: &RuntimeError) {
    tracing::debug!(%error, "onnx-scanner: call rejected before execution");
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
        "onnx-scanner: model unavailable; the gear will start without it"
    );
    ModelEntry::Unavailable(failure.reason)
}

/// Loads one model and its tokenizer, classifying whichever step fails.
async fn load_one(
    cfg: &ModelConfig,
    metrics: &Arc<dyn RuntimeMetricsPort>,
) -> Result<LoadedEntry, LoadError> {
    let model = LoadedModel::load(cfg)
        .await?
        .with_metrics(Arc::clone(metrics));
    let tokenizer_path = cfg.model_dir.join(&cfg.tokenizer_file);
    let tokenizer = LoadedTokenizer::load(&tokenizer_path)
        .map_err(|source| {
            LoadError::Failed(LoadFailure {
                reason: ModelFailureReason::TokenizerInvalid,
                source,
            })
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

/// In-process implementation of [`ModelRuntime`], built by `onnx-scanner` at
/// `init`.
pub struct LocalRuntime {
    /// Every *configured* model, not only the ones that loaded.
    models: BTreeMap<String, ModelEntry>,
}

impl LocalRuntime {
    /// Builds the runtime over an already-loaded model map.
    ///
    /// `#[doc(hidden)] pub` only so the integration tests can drive the real
    /// `list_models` projection rather than a reimplementation of it.
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
    fn check_text(entry: &LoadedEntry, model_id: &str, text: &str) -> Result<(), RuntimeError> {
        let limit = entry.model.limits().text_bytes;
        if text.len() > limit {
            return Err(RuntimeError::InvalidInput {
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
    fn ready_entry(&self, model_id: &str) -> Result<&LoadedEntry, RuntimeError> {
        match self.models.get(model_id) {
            Some(ModelEntry::Ready(entry)) => Ok(entry),
            Some(ModelEntry::Unavailable(reason)) => Err(RuntimeError::ModelUnavailable {
                model_id: model_id.to_owned(),
                reason: *reason,
            }),
            None => Err(RuntimeError::UnknownModel {
                model_id: model_id.to_owned(),
            }),
        }
    }
}

#[async_trait]
impl ModelRuntime for LocalRuntime {
    #[tracing::instrument(
        name = "onnx_scanner.runtime.run",
        skip_all,
        fields(model_id = %model_id, tensors = inputs.len())
    )]
    async fn run(
        &self,
        model_id: &str,
        inputs: Vec<NamedTensor>,
    ) -> Result<Vec<NamedTensor>, RuntimeError> {
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
        name = "onnx_scanner.runtime.encode",
        skip_all,
        fields(model_id = %model_id, text_bytes = text.len())
    )]
    async fn encode(&self, model_id: &str, text: &str) -> Result<TokenEncoding, RuntimeError> {
        let entry = self
            .ready_entry(model_id)
            .inspect_err(log_dispatch_failure)?;
        Self::check_text(entry, model_id, text).inspect_err(log_dispatch_failure)?;
        entry.tokenizer.encode(model_id, text.to_owned()).await
    }

    #[tracing::instrument(
        name = "onnx_scanner.runtime.encode_fragment",
        skip_all,
        fields(model_id = %model_id, text_bytes = fragment.len())
    )]
    async fn encode_fragment(
        &self,
        model_id: &str,
        fragment: &str,
    ) -> Result<Vec<i64>, RuntimeError> {
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
    async fn list_models(&self) -> Result<Vec<ModelInfo>, RuntimeError> {
        Ok(self.model_snapshot())
    }
}

impl LocalRuntime {
    /// Every configured model's current state, read now rather than at load.
    ///
    /// Shared by [`ModelRuntime::list_models`] and the platform
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
