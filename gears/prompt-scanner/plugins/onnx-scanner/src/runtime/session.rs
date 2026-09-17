//! The ONLY file in this module that touches `ort` types. Session load (with
//! `GraphContract` + SHA-256 verification), a pool of replicas per model
//! (see `docs/runtime/DESIGN.md`), and generic tensor in/out — no model-family
//! decoding logic belongs here.
//!
//! The session itself — one pinned `ort`, an open under a deadline on an
//! abandonable thread, exclusive use of one session — is the toolkit's
//! (`toolkit_onnx_runtime::OnnxSession`). What this file adds on top is what
//! that crate deliberately leaves to the gear: the replica pool, integrity and
//! contract verification, input bounds, the per-inference timeout and load
//! shedding, and rebuilding a replica lost to a panic.
//!
//! # Concurrency model
//!
//! ONNX Runtime's `Run` is not thread-safe — `ort` takes `&mut self` on
//! [`Session::run`] deliberately — so a replica must be held exclusively for
//! the duration of an inference. Replicas are the only concurrency mechanism
//! available, and each is guarded by a [`tokio::sync::Mutex`] rather than a
//! `std::sync::Mutex`: callers queue fairly on the async side instead of
//! parking an executor thread, and the guard is `'static` so it can move into
//! [`tokio::task::spawn_blocking`], where the CPU-bound work belongs.
//!
//! Each slot holds an `Option<Session>`. The blocking task takes the session
//! out, runs, and puts it back. An unwind therefore leaves the slot empty
//! rather than leaving behind a session whose ONNX state is unknown, and the
//! next caller rebuilds it from the same verified spec.
//!
//! This mirrors `file-parser`'s `infra/magika_detector.rs`, which solves the
//! same problem in this repo.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ort::session::Session;
use ort::value::{Tensor, Value};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use toolkit_onnx_runtime::ort;
use toolkit_onnx_runtime::{OnnxSession, OpenError, SessionOptions};

use super::{
    LoadedModelInfo, ModelFailureReason, ModelInfo, ModelState, NamedTensor, RuntimeError,
    TensorData,
};

use super::config::{ModelConfig, OutputDtype, OverloadPolicy};
use super::metrics_port::{InferenceOutcome, NoopMetrics, RuntimeMetricsPort};
use super::registry::{
    InputLimits, ReplicaSelector, validate_tensor, verify_checksum, verify_contract,
    verify_input_names,
};

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// How long one session may take to open before the runtime is declared hung.
///
/// Per session, and generous: a healthy open maps a several-hundred-megabyte
/// model and an unoptimized build is slower still. The hang never completes,
/// so the only cost of a generous budget is how long a genuinely wedged
/// startup takes to report itself.
const SESSION_OPEN_TIMEOUT: Duration = Duration::from_mins(1);

fn load_failed(model_id: &str, err: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::Internal(format!("failed to load model {model_id}: {err}"))
}

/// A load failure, with the classification a consumer branches on.
///
/// The `reason` is stable and payload-free; `source` carries the detail, for
/// the operator reading startup logs.
#[derive(Debug, thiserror::Error)]
#[error("{source}")]
pub struct LoadFailure {
    pub reason: ModelFailureReason,
    #[source]
    pub source: RuntimeError,
}

impl LoadFailure {
    fn new(reason: ModelFailureReason, source: RuntimeError) -> Self {
        Self { reason, source }
    }
}

/// Why [`LoadedModel::load`] did not produce a model.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// This model cannot be served; others may be. See [`LoadFailure`].
    #[error("{0}")]
    Failed(LoadFailure),
    /// ONNX Runtime did not load in time. The thread that tried is abandoned,
    /// so the process must terminate rather than carry on or retry: this is
    /// never a per-model failure.
    #[error(
        "ONNX Runtime did not load within {seconds}s (check ORT_DYLIB_PATH); the process must \
         terminate"
    )]
    RuntimeHung { seconds: u64 },
}

impl From<LoadFailure> for LoadError {
    fn from(failure: LoadFailure) -> Self {
        Self::Failed(failure)
    }
}

/// One replica slot. `None` means the previous occupant was lost to a panic
/// and the next caller must rebuild before using it.
type Replica = Arc<Mutex<Option<OnnxSession>>>;

/// `ort` takes session inputs as `(name, value)` pairs with a borrowed-or-owned
/// name.
type SessionInput = (std::borrow::Cow<'static, str>, Value);

/// Everything needed to build a session for one model, kept so a replica
/// rebuilt after a panic gets exactly the settings the startup build used.
struct SessionSpec {
    model_id: String,
    model_path: PathBuf,
    intra_threads: usize,
}

/// Maps ONNX Runtime's element type onto the dtypes this gear can extract.
/// `None` means the graph emits something we have no extraction path for,
/// which is a load-time failure rather than a runtime surprise.
fn dtype_of(ty: ort::value::TensorElementType) -> Option<OutputDtype> {
    use ort::value::TensorElementType as T;
    match ty {
        T::Float32 => Some(OutputDtype::F32),
        T::Int64 => Some(OutputDtype::I64),
        T::Uint8 => Some(OutputDtype::U8),
        T::Bool => Some(OutputDtype::Bool),
        _ => None,
    }
}

/// Verifies every declared output dtype against what the graph actually emits.
///
/// This is the load-time half of "no silent reinterpretation": the session's
/// own output metadata is available here, so a model declaring `f32` for an
/// output the graph emits as `i64` fails at startup rather than handing a
/// caller plausible, catastrophically wrong numbers.
fn verify_output_dtypes(
    model_id: &str,
    session: &Session,
    declared: &BTreeMap<String, OutputDtype>,
) -> Result<(), RuntimeError> {
    for output in session.outputs() {
        let name = output.name();
        let Some(want) = declared.get(name).copied() else {
            return Err(RuntimeError::ContractMismatch {
                model_id: model_id.to_owned(),
                message: format!(
                    "output '{name}' has no `output_dtypes` entry; every declared output needs \
                     one, because guessing is how a tensor gets reinterpreted"
                ),
            });
        };

        let ort::value::ValueType::Tensor { ty, .. } = output.dtype() else {
            return Err(RuntimeError::ContractMismatch {
                model_id: model_id.to_owned(),
                message: format!("output '{name}' is not a tensor; this gear only handles tensors"),
            });
        };

        match dtype_of(*ty) {
            Some(actual) if actual == want => {}
            Some(actual) => {
                return Err(RuntimeError::ContractMismatch {
                    model_id: model_id.to_owned(),
                    message: format!(
                        "output '{name}' is declared {want:?} but the graph emits {actual:?}"
                    ),
                });
            }
            None => {
                return Err(RuntimeError::ContractMismatch {
                    model_id: model_id.to_owned(),
                    message: format!(
                        "output '{name}' has element type {ty:?}, which this gear cannot extract"
                    ),
                });
            }
        }
    }
    Ok(())
}

/// Reads `id2label` from the model's metadata file into class-id order.
///
/// Deliberately infallible from the caller's point of view: this is optional
/// metadata, and a model that runs correctly must not be refused because a
/// sidecar JSON is absent or malformed. Every failure path returns `None` and
/// logs, so the model still serves — a consumer that needs labels checks for
/// them and says so in its own terms.
///
/// A non-contiguous map is rejected rather than padded. Consumers index this
/// positionally by class id, so a gap would silently shift every label after
/// it — mislabelling is worse than having no labels at all.
fn read_labels(path: &std::path::Path, model_id: &str) -> Option<Vec<String>> {
    let raw = std::fs::read_to_string(path).ok()?;

    let parsed: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(model_id, error = %e, "labels file is not valid JSON; no labels");
            return None;
        }
    };

    let map = parsed.get("id2label")?.as_object()?;

    let mut indexed: Vec<(usize, String)> = map
        .iter()
        .filter_map(|(k, v)| Some((k.parse().ok()?, v.as_str()?.to_owned())))
        .collect();
    indexed.sort_unstable_by_key(|(i, _)| *i);

    if indexed.len() != map.len() || !indexed.iter().enumerate().all(|(n, (i, _))| n == *i) {
        tracing::warn!(
            model_id,
            entries = map.len(),
            "id2label is not a contiguous 0..n map; no labels"
        );
        return None;
    }

    Some(indexed.into_iter().map(|(_, label)| label).collect())
}

/// Opens one session through the toolkit: on an abandonable thread, under a
/// deadline, so a bad `ORT_DYLIB_PATH` cannot wedge the process.
async fn open_session(spec: &SessionSpec) -> Result<OnnxSession, OpenError> {
    let mut options = SessionOptions::new(&spec.model_path);
    options.intra_op_threads = Some(spec.intra_threads);
    options.load_timeout = SESSION_OPEN_TIMEOUT;
    options.thread_name = format!("onnx-init-{}", spec.model_id);
    OnnxSession::open(&options).await
}

/// The input names a model file declares, as the runtime sees them.
///
/// Opens a throwaway session at the same settings a served one gets, because
/// the declared inputs can depend on the graph optimization level the session
/// is built at — the answer is the runtime's, not something to probe from the
/// file. For tooling that has to state a contract before a model is configured
/// (the quality evaluation); the host itself reads the same names off the
/// session it serves.
///
/// # Errors
///
/// [`RuntimeError::Internal`] when the runtime refuses the file or does not
/// load in time.
pub async fn declared_input_names(
    model_path: &std::path::Path,
) -> Result<Vec<String>, RuntimeError> {
    let model_id = model_path.display().to_string();
    let mut options = SessionOptions::new(model_path);
    options.load_timeout = SESSION_OPEN_TIMEOUT;
    let session = OnnxSession::open(&options)
        .await
        .map_err(|e| load_failed(&model_id, e))?;
    Ok(session
        .run(|session| {
            session
                .inputs()
                .iter()
                .map(|input| input.name().to_owned())
                .collect()
        })
        .await)
}

/// One configured model: N loaded session replicas, dispatched round-robin
/// (see `docs/DESIGN.md`'s "pool of replicas, not a single mutex" decision).
pub struct LoadedModel {
    pub info: ModelInfo,
    /// Kept alongside `info` because the request path needs them on every
    /// call, and `info.loaded` is an `Option` that only introspection should
    /// have to unwrap.
    output_names: Arc<[String]>,
    /// Set once every replica has been lost and a rebuild has failed, so the
    /// model reports itself unavailable instead of failing every call with a
    /// fresh rebuild error.
    ///
    /// One-way: nothing here brings a model back, because nothing here can
    /// tell whether the underlying cause was fixed. Recovery is a restart, or
    /// the reload this design deliberately defers.
    session_build_failed: Arc<AtomicBool>,
    replicas: Vec<Replica>,
    /// Bounds concurrent inferences. Sized from the replica count by default:
    /// more in flight than there are replicas is queueing, not parallelism,
    /// and unbounded queueing on a process-wide blocking pool is what this
    /// exists to prevent.
    in_flight: Arc<tokio::sync::Semaphore>,
    limits: InputLimits,
    input_names: Vec<String>,
    inference_timeout: Duration,
    on_overload: OverloadPolicy,
    metrics: Arc<dyn RuntimeMetricsPort>,
    selector: ReplicaSelector,
    output_dtypes: Arc<BTreeMap<String, OutputDtype>>,
    spec: Arc<SessionSpec>,
}

impl LoadedModel {
    /// Reads and verifies the model file, then builds `cfg.replicas` sessions
    /// from it.
    ///
    /// # Errors
    ///
    /// Returns a [`LoadFailure`] carrying the classification of what went
    /// wrong — an unreadable file, a checksum or contract mismatch, an invalid
    /// config, or a session that would not build.
    pub async fn load(cfg: &ModelConfig) -> Result<Self, LoadError> {
        // Validated rather than coerced with `.max(1)`: a config asking for
        // zero replicas is a mistake worth naming, not a value to guess at.
        // Checked before any I/O so a bad config fails immediately instead of
        // after reading a multi-hundred-megabyte model file.
        let replica_count = NonZeroUsize::new(cfg.replicas).ok_or_else(|| {
            LoadFailure::new(
                ModelFailureReason::InvalidConfig,
                RuntimeError::Internal(format!(
                    "model {}: `replicas` must be at least 1, got 0",
                    cfg.model_id
                )),
            )
        })?;

        // Checked before any I/O, alongside the replica count: a config that
        // cannot produce a correct result should not cost a 400 MB read first.
        for output in &cfg.expected_outputs {
            if !cfg.output_dtypes.contains_key(output) {
                return Err(LoadFailure::new(
                    ModelFailureReason::InvalidConfig,
                    RuntimeError::Internal(format!(
                        "model {}: output '{output}' is in `expected_outputs` but missing from \
                         `output_dtypes`; every declared output needs a dtype, because guessing \
                         is how a tensor gets reinterpreted",
                        cfg.model_id
                    )),
                )
                .into());
            }
        }

        let model_path = cfg.model_dir.join(&cfg.model_file);

        // Reading and hashing a model of several hundred megabytes is blocking
        // work: off the async workers.
        let hashed = {
            let model_path = model_path.clone();
            tokio::task::spawn_blocking(move || {
                std::fs::read(&model_path).map(|bytes| sha256_hex(&bytes))
            })
            .await
        };
        let actual_hash = match hashed {
            Ok(Ok(hash)) => hash,
            Ok(Err(e)) => {
                return Err(LoadFailure::new(
                    ModelFailureReason::FileUnreadable,
                    load_failed(
                        &cfg.model_id,
                        format!("reading {}: {e}", model_path.display()),
                    ),
                )
                .into());
            }
            Err(join_err) => {
                return Err(LoadFailure::new(
                    ModelFailureReason::FileUnreadable,
                    load_failed(&cfg.model_id, format!("hashing the model file: {join_err}")),
                )
                .into());
            }
        };
        if !cfg.skip_checksum {
            verify_checksum(&cfg.model_id, &cfg.sha256, &actual_hash).map_err(|e| {
                LoadFailure::new(
                    ModelFailureReason::ChecksumMismatch,
                    RuntimeError::Internal(e.to_string()),
                )
            })?;
        }

        let spec = Arc::new(SessionSpec {
            model_id: cfg.model_id.clone(),
            model_path,
            intra_threads: cfg.intra_threads,
        });

        let mut replicas = Vec::with_capacity(replica_count.get());
        let mut actual_inputs = Vec::new();
        let mut actual_outputs = Vec::new();

        for i in 0..replica_count.get() {
            let session = open_session(&spec).await.map_err(|e| match e {
                OpenError::RuntimeHung { seconds } => LoadError::RuntimeHung { seconds },
                other => LoadError::Failed(LoadFailure::new(
                    ModelFailureReason::SessionBuildFailed,
                    load_failed(&cfg.model_id, other),
                )),
            })?;

            if i == 0 {
                // The graph's own metadata: the contract is checked against
                // what the runtime reports, never against what the file name
                // promises.
                let (inputs, outputs) = session
                    .run(|session| {
                        verify_output_dtypes(&cfg.model_id, session, &cfg.output_dtypes).map(|()| {
                            (
                                session
                                    .inputs()
                                    .iter()
                                    .map(|input| input.name().to_owned())
                                    .collect::<Vec<_>>(),
                                session
                                    .outputs()
                                    .iter()
                                    .map(|output| output.name().to_owned())
                                    .collect::<Vec<_>>(),
                            )
                        })
                    })
                    .await
                    .map_err(|e| LoadFailure::new(ModelFailureReason::ContractMismatch, e))?;
                actual_inputs = inputs;
                actual_outputs = outputs;
                verify_contract(
                    &cfg.model_id,
                    &cfg.expected_inputs,
                    &cfg.expected_outputs,
                    &actual_inputs,
                    &actual_outputs,
                )
                .map_err(|e| {
                    LoadFailure::new(
                        ModelFailureReason::ContractMismatch,
                        RuntimeError::ContractMismatch {
                            model_id: cfg.model_id.clone(),
                            message: e.to_string(),
                        },
                    )
                })?;
            }

            replicas.push(Arc::new(Mutex::new(Some(session))));
        }

        let output_names = actual_outputs.clone();
        Ok(Self {
            info: ModelInfo {
                model_id: cfg.model_id.clone(),
                state: ModelState::Ready,
                loaded: Some(LoadedModelInfo {
                    input_names: actual_inputs,
                    output_names: actual_outputs,
                    sha256: actual_hash,
                    replicas: replica_count.get(),
                    labels: read_labels(&cfg.model_dir.join(&cfg.labels_file), &cfg.model_id),
                }),
            },
            output_names: output_names.into(),
            session_build_failed: Arc::new(AtomicBool::new(false)),
            replicas,
            in_flight: Arc::new(tokio::sync::Semaphore::new(
                cfg.max_in_flight.unwrap_or(replica_count.get()).max(1),
            )),
            limits: InputLimits {
                elements_per_tensor: cfg.max_tensor_elements,
                tensors_per_call: cfg.max_input_tensors,
                extent_per_dimension: cfg.max_dimension,
                text_bytes: cfg.max_text_bytes,
            },
            input_names: cfg.expected_inputs.clone(),
            inference_timeout: cfg.inference_timeout,
            on_overload: cfg.on_overload,
            // Replaced by `with_metrics` at gear init; a model built in a test
            // records nothing rather than requiring a meter.
            metrics: Arc::new(NoopMetrics),
            selector: ReplicaSelector::new(replica_count),
            output_dtypes: Arc::new(cfg.output_dtypes.clone()),
            spec,
        })
    }

    /// Attaches the metrics port. Called once at gear init, before the model
    /// is shared.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<dyn RuntimeMetricsPort>) -> Self {
        self.metrics = metrics;
        self
    }

    /// This model's state right now.
    ///
    /// [`ModelState::Ready`] until every replica has been lost and a rebuild
    /// has failed; see `session_build_failed`.
    #[must_use]
    pub fn state(&self) -> ModelState {
        if self.session_build_failed.load(Ordering::Relaxed) {
            ModelState::Unavailable {
                reason: ModelFailureReason::SessionBuildFailed,
            }
        } else {
            ModelState::Ready
        }
    }

    /// How many replicas currently hold a session.
    ///
    /// A best-effort snapshot: a replica busy serving a request is counted as
    /// live (its session is checked out, not lost), and one being rebuilt may
    /// read either way. Introspection only — never a gate on serving.
    #[must_use]
    pub fn live_replicas(&self) -> usize {
        self.replicas
            .iter()
            .filter(|replica| replica.try_lock().map_or(true, |guard| guard.is_some()))
            .count()
    }

    /// This model's input limits, so the gear can validate `encode` text
    /// without reaching into the session layer.
    #[must_use]
    pub const fn limits(&self) -> InputLimits {
        self.limits
    }

    /// Runs one inference on the next replica in round-robin order.
    ///
    /// Inputs are validated before anything is allocated, the call is bounded
    /// by the model's `inference_timeout`, and concurrency is capped.
    ///
    /// # Cancellation and timeouts
    ///
    /// Cancel-safe with respect to the session: dropping the returned future
    /// does not stop the in-flight inference — its result is simply discarded
    /// — and the replica is released either way, because the blocking task
    /// owns the mutex guard and drops it on completion.
    ///
    /// **A timeout abandons the result; it does not cancel the work.** ONNX
    /// Runtime has no mid-graph cancellation, so after
    /// [`RuntimeError::Timeout`] the replica stays occupied until the
    /// underlying inference finishes on its own. Retrying immediately competes
    /// with work that is still running; a caller that times out repeatedly is
    /// making the problem worse, not shedding it.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::InvalidInput`] for a malformed or oversized input;
    /// [`RuntimeError::ContractMismatch`] if the supplied tensor names do
    /// not match the model's declared inputs; [`RuntimeError::Overloaded`]
    /// when at capacity under [`OverloadPolicy::Reject`];
    /// [`RuntimeError::Timeout`] on deadline expiry;
    /// [`RuntimeError::ModelUnavailable`] if every replica has been lost;
    /// [`RuntimeError::InferenceFailed`] if the session rejects the inputs
    /// or an output cannot be extracted as its declared dtype.
    #[tracing::instrument(
        name = "onnx_scanner.runtime.run",
        skip_all,
        fields(
            model_id = %self.info.model_id,
            tensors = inputs.len(),
            replicas = self.replicas.len(),
            outcome = tracing::field::Empty,
            queue_wait_ms = tracing::field::Empty,
            execution_ms = tracing::field::Empty,
        )
    )]
    pub async fn run(&self, inputs: Vec<NamedTensor>) -> Result<Vec<NamedTensor>, RuntimeError> {
        // `skip_all` above is deliberate and load-bearing: `inputs` carries
        // tensor data. Only the count is recorded, never the values.
        let span = tracing::Span::current();
        self.metrics.record_in_flight_delta(&self.info.model_id, 1);
        let finished = InFlightGuard {
            metrics: &*self.metrics,
            model_id: &self.info.model_id,
        };

        let outcome = self.run_instrumented(inputs, &span).await;
        drop(finished);
        outcome
    }

    /// The body of [`Self::run`], separated so the in-flight guard in the
    /// caller covers every early return including validation failures.
    async fn run_instrumented(
        &self,
        inputs: Vec<NamedTensor>,
        span: &tracing::Span,
    ) -> Result<Vec<NamedTensor>, RuntimeError> {
        let record = |outcome: InferenceOutcome, queue_wait: Duration, execution: Duration| {
            span.record("outcome", outcome.as_label());
            span.record("queue_wait_ms", queue_wait.as_millis());
            span.record("execution_ms", execution.as_millis());
            self.metrics
                .record_inference(&self.info.model_id, outcome, queue_wait, execution);
        };

        if let Err(e) = self.validate_inputs(&inputs) {
            record((&e).into(), Duration::ZERO, Duration::ZERO);
            return Err(e);
        }

        // Queue wait is measured around permit acquisition alone. That is what
        // makes it distinguishable from execution: it rises with concurrency
        // and replica scarcity, where execution time rises with input size and
        // thread budget. A single total would conceal which one is the problem.
        let queueing = Instant::now();
        let permit = match self.acquire_permit().await {
            Ok(permit) => permit,
            Err(e) => {
                record((&e).into(), queueing.elapsed(), Duration::ZERO);
                return Err(e);
            }
        };
        let queue_wait = queueing.elapsed();

        let model_id = self.info.model_id.clone();
        let output_names = Arc::clone(&self.output_names);
        let output_dtypes = Arc::clone(&self.output_dtypes);
        let timeout = self.inference_timeout;

        let executing = Instant::now();
        let inference = self.with_replica(move |session| {
            let session_inputs = build_inputs(&inputs)?;
            let outputs =
                session
                    .run(session_inputs)
                    .map_err(|e| RuntimeError::InferenceFailed {
                        model_id: model_id.clone(),
                        message: e.to_string(),
                    })?;
            collect_outputs(&model_id, &output_names, &output_dtypes, &outputs)
        });

        let (result, replica_wait) = match tokio::time::timeout(timeout, inference).await {
            Ok(Ok((value, replica_wait))) => (Ok(value), replica_wait),
            Ok(Err(e)) => (Err(e), Duration::ZERO),
            Err(_elapsed) => (
                Err(RuntimeError::Timeout {
                    model_id: self.info.model_id.clone(),
                    elapsed_ms: u64::try_from(executing.elapsed().as_millis()).unwrap_or(u64::MAX),
                }),
                Duration::ZERO,
            ),
        };
        drop(permit);

        // Waiting for a replica is queueing, not execution — even though it
        // happens after the permit was granted. Attributing it to execution
        // would make a contended single-replica model look slow rather than
        // oversubscribed, which is the opposite diagnosis.
        let queue_wait = queue_wait + replica_wait;
        let execution = executing.elapsed().saturating_sub(replica_wait);
        match &result {
            Ok(_) => record(InferenceOutcome::Ok, queue_wait, execution),
            Err(e) => record(e.into(), queue_wait, execution),
        }
        result
    }

    /// Rejects malformed or oversized input before any allocation.
    fn validate_inputs(&self, inputs: &[NamedTensor]) -> Result<(), RuntimeError> {
        if inputs.len() > self.limits.tensors_per_call {
            return Err(RuntimeError::InvalidInput {
                model_id: self.info.model_id.clone(),
                tensor: "<call>".to_owned(),
                message: format!(
                    "{} input tensors, over the limit of {}",
                    inputs.len(),
                    self.limits.tensors_per_call
                ),
            });
        }

        for tensor in inputs {
            let element_count = match &tensor.data {
                TensorData::I64(d) => d.len(),
                TensorData::F32(d) => d.len(),
                TensorData::U8(d) => d.len(),
                TensorData::Bool(d) => d.len(),
            };
            validate_tensor(&tensor.name, &tensor.shape, element_count, self.limits).map_err(
                |rejection| RuntimeError::InvalidInput {
                    model_id: self.info.model_id.clone(),
                    tensor: rejection.tensor,
                    message: rejection.message,
                },
            )?;
        }

        // Names last: a caller that got the shapes wrong should hear about the
        // shapes, which are the more specific complaint.
        let supplied: Vec<String> = inputs.iter().map(|t| t.name.clone()).collect();
        verify_input_names(&self.info.model_id, &self.input_names, &supplied).map_err(|e| {
            RuntimeError::ContractMismatch {
                model_id: self.info.model_id.clone(),
                message: e.to_string(),
            }
        })
    }

    /// Takes an in-flight permit, or sheds the call.
    async fn acquire_permit(&self) -> Result<tokio::sync::OwnedSemaphorePermit, RuntimeError> {
        let overloaded = || RuntimeError::Overloaded {
            model_id: self.info.model_id.clone(),
            in_flight: self.in_flight_capacity(),
        };

        match self.on_overload {
            OverloadPolicy::Reject => Arc::clone(&self.in_flight)
                .try_acquire_owned()
                .map_err(|_| overloaded()),
            // Bounded by the same deadline the inference gets, so a queued
            // caller cannot wait longer than a running one is allowed to take.
            OverloadPolicy::Queue => {
                match tokio::time::timeout(
                    self.inference_timeout,
                    Arc::clone(&self.in_flight).acquire_owned(),
                )
                .await
                {
                    Ok(Ok(permit)) => Ok(permit),
                    // The semaphore is never closed, so this is unreachable in
                    // practice; treat it as capacity exhaustion rather than
                    // panicking on an impossible branch.
                    Ok(Err(_closed)) => Err(overloaded()),
                    Err(_elapsed) => Err(overloaded()),
                }
            }
        }
    }

    /// The configured concurrency ceiling for this model.
    #[must_use]
    fn in_flight_capacity(&self) -> usize {
        self.replicas.len()
    }

    /// Takes exclusive use of one replica and runs `f` on the blocking pool.
    ///
    /// The session never leaves the mutex from a caller's point of view; the
    /// *guard* moves into the closure. That is what makes cancellation safe —
    /// the guard is released by `Drop` when the blocking task ends, whether or
    /// not anyone still awaits the result.
    /// Returns the closure's result alongside **how long acquiring the replica
    /// took**, so the caller can attribute that time to queueing rather than to
    /// execution. The two are indistinguishable from the outside, and
    /// conflating them inverts the diagnosis: contention reads as a slow model.
    async fn with_replica<T, F>(&self, f: F) -> Result<(T, Duration), RuntimeError>
    where
        F: FnOnce(&mut Session) -> Result<T, RuntimeError> + Send + 'static,
        T: Send + 'static,
    {
        // `next_index` returns a value in `0..replica_count`, and `selector`
        // was constructed from `replicas.len()`, which never changes. The
        // index is therefore always in bounds.
        // A model that has already lost every replica reports itself
        // unavailable rather than making each caller rediscover it.
        if self.session_build_failed.load(Ordering::Relaxed) {
            return Err(RuntimeError::ModelUnavailable {
                model_id: self.info.model_id.clone(),
                reason: ModelFailureReason::SessionBuildFailed,
            });
        }

        let idx = self.selector.next_index();
        let replica = Arc::clone(&self.replicas[idx]);

        // Fair FIFO queueing, on the async side rather than on a blocking
        // thread. `lock_owned` because `spawn_blocking` needs `'static`.
        let waiting = Instant::now();
        let mut guard = replica.lock_owned().await;
        let replica_wait = waiting.elapsed();

        if guard.is_none() {
            // A previous inference on this replica panicked and left the slot
            // empty. Rebuild before anyone gets to use it.
            match Self::rebuild(&self.spec).await {
                Ok(session) => {
                    self.metrics
                        .record_session_rebuild(&self.info.model_id, true);
                    *guard = Some(session);
                }
                Err(e) => {
                    self.metrics
                        .record_session_rebuild(&self.info.model_id, false);
                    // One failed rebuild is not fatal while other replicas are
                    // still serving — but if this was the last one, the model
                    // is gone and must say so rather than failing every future
                    // call with a fresh rebuild error.
                    drop(guard);
                    if self.live_replicas() == 0 {
                        self.session_build_failed.store(true, Ordering::Relaxed);
                        tracing::error!(
                            model_id = %self.info.model_id,
                            reason = ModelFailureReason::SessionBuildFailed.as_code(),
                            error = %e,
                            "onnx-scanner: model lost every replica and could not rebuild; \
                             it is now unavailable"
                        );
                    }
                    return Err(e);
                }
            }
        }

        let model_id = self.info.model_id.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = guard;
            // `take` so an unwind inside `f` leaves the slot empty instead of
            // a session whose ONNX state is unknown.
            let Some(session) = guard.take() else {
                return Err(RuntimeError::Internal(format!(
                    "replica for model {model_id} was empty after rebuild"
                )));
            };
            // `run_blocking`: this is a blocking-pool thread, and the replica
            // is already held exclusively by `guard`, so the session's own
            // lock is uncontended.
            let result = session.run_blocking(f);
            // Restored on success and on a returned error alike — a rejected
            // input does not damage the session. Only a panic skips this,
            // which is exactly when the session must not be reused.
            *guard = Some(session);
            result.map(|value| (value, replica_wait))
        })
        .await
        .map_err(|join_err| {
            RuntimeError::Internal(format!(
                "inference task for model {} did not complete: {join_err}",
                self.info.model_id
            ))
        })?
    }

    /// Opens a fresh session at the same settings the startup open used.
    ///
    /// A runtime that hangs here is reported as a failed rebuild: the model
    /// goes unavailable (and says so) rather than wedging callers. The thread
    /// the toolkit abandons is logged by the toolkit; recovery is a restart.
    async fn rebuild(spec: &Arc<SessionSpec>) -> Result<OnnxSession, RuntimeError> {
        open_session(spec)
            .await
            .map_err(|e| load_failed(&spec.model_id, e))
    }
}

fn build_inputs(inputs: &[NamedTensor]) -> Result<Vec<SessionInput>, RuntimeError> {
    let mut session_inputs = Vec::with_capacity(inputs.len());
    for tensor in inputs {
        let value = build_tensor(tensor)?;
        session_inputs.push((tensor.name.clone().into(), value));
    }
    Ok(session_inputs)
}

fn collect_outputs(
    model_id: &str,
    output_names: &[String],
    output_dtypes: &BTreeMap<String, OutputDtype>,
    outputs: &ort::session::SessionOutputs<'_>,
) -> Result<Vec<NamedTensor>, RuntimeError> {
    let mut result = Vec::with_capacity(output_names.len());
    for name in output_names {
        // No default: `LoadedModel::load` rejects a config that omits any
        // declared output, and `verify_output_dtypes` proves the declaration
        // matches the graph. An absent entry here means those checks were
        // bypassed, which is a bug, not a tensor to guess at.
        let dtype = output_dtypes.get(name).copied().ok_or_else(|| {
            RuntimeError::Internal(format!(
                "model {model_id}: output '{name}' reached extraction with no declared dtype"
            ))
        })?;
        let value = outputs
            .get(name.as_str())
            .ok_or_else(|| RuntimeError::InferenceFailed {
                model_id: model_id.to_owned(),
                message: format!("session did not return expected output '{name}'"),
            })?;
        result.push(extract_tensor(model_id, name, value, dtype)?);
    }
    Ok(result)
}

fn build_tensor(tensor: &NamedTensor) -> Result<Value, RuntimeError> {
    // Checked rather than asserted in a comment: `validate_tensor` bounds each
    // dimension by `extent_per_dimension`, but that limit is operator-set, so
    // the conversion proves itself instead of trusting configuration.
    let shape: Vec<i64> = tensor
        .shape
        .iter()
        .map(|&d| {
            i64::try_from(d).map_err(|_| {
                RuntimeError::Internal(format!(
                    "tensor '{}': dimension {d} does not fit in i64",
                    tensor.name
                ))
            })
        })
        .collect::<Result<_, _>>()?;
    let value = match &tensor.data {
        TensorData::I64(data) => Tensor::from_array((shape, data.clone()))
            .map_err(|e| {
                RuntimeError::Internal(format!("building I64 tensor '{}': {e}", tensor.name))
            })?
            .into_dyn(),
        TensorData::F32(data) => Tensor::from_array((shape, data.clone()))
            .map_err(|e| {
                RuntimeError::Internal(format!("building F32 tensor '{}': {e}", tensor.name))
            })?
            .into_dyn(),
        TensorData::U8(data) => Tensor::from_array((shape, data.clone()))
            .map_err(|e| {
                RuntimeError::Internal(format!("building U8 tensor '{}': {e}", tensor.name))
            })?
            .into_dyn(),
        TensorData::Bool(data) => Tensor::from_array((shape, data.clone()))
            .map_err(|e| {
                RuntimeError::Internal(format!("building Bool tensor '{}': {e}", tensor.name))
            })?
            .into_dyn(),
    };
    Ok(value)
}

fn extract_tensor(
    model_id: &str,
    name: &str,
    value: &Value,
    dtype: OutputDtype,
) -> Result<NamedTensor, RuntimeError> {
    let fail = |e: ort::Error| RuntimeError::InferenceFailed {
        model_id: model_id.to_owned(),
        message: format!("extracting output '{name}': {e}"),
    };
    let (shape, data) = match dtype {
        OutputDtype::F32 => {
            let (shape, slice) = value.try_extract_tensor::<f32>().map_err(fail)?;
            (shape, TensorData::F32(slice.to_vec()))
        }
        OutputDtype::I64 => {
            let (shape, slice) = value.try_extract_tensor::<i64>().map_err(fail)?;
            (shape, TensorData::I64(slice.to_vec()))
        }
        OutputDtype::U8 => {
            let (shape, slice) = value.try_extract_tensor::<u8>().map_err(fail)?;
            (shape, TensorData::U8(slice.to_vec()))
        }
        OutputDtype::Bool => {
            let (shape, slice) = value.try_extract_tensor::<bool>().map_err(fail)?;
            (shape, TensorData::Bool(slice.to_vec()))
        }
    };
    // A materialized tensor's dims are non-negative, but that is ONNX
    // Runtime's invariant rather than ours, so it is checked here.
    let shape = shape
        .iter()
        .map(|&d| {
            usize::try_from(d).map_err(|_| RuntimeError::InferenceFailed {
                model_id: model_id.to_owned(),
                message: format!("output '{name}' reported a negative dimension {d}"),
            })
        })
        .collect::<Result<_, _>>()?;
    Ok(NamedTensor {
        name: name.to_owned(),
        shape,
        data,
    })
}

/// Decrements the in-flight gauge on drop, so a cancelled call cannot leave it
/// permanently elevated — the failure mode that makes a gauge untrustworthy.
struct InFlightGuard<'a> {
    metrics: &'a dyn RuntimeMetricsPort,
    model_id: &'a str,
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        self.metrics.record_in_flight_delta(self.model_id, -1);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::use_debug)]
mod tests {
    use std::path::PathBuf;

    use super::LoadedModel;
    use crate::runtime::config::ModelConfig;

    /// Zero replicas is a configuration mistake worth naming, not a value to
    /// silently coerce to one. Rejected before any I/O, so this needs no model
    /// file on disk.
    #[tokio::test]
    async fn zero_replicas_fails_to_load_naming_model_and_field() {
        let cfg = ModelConfig {
            model_id: "zero-replica-model".to_owned(),
            model_dir: PathBuf::from("/nonexistent"),
            skip_checksum: true,
            replicas: 0,
            ..ModelConfig::default()
        };

        let Err(err) = LoadedModel::load(&cfg).await else {
            panic!("zero replicas must not load");
        };
        let message = err.to_string();
        assert!(
            message.contains("zero-replica-model"),
            "error names the model: {message}"
        );
        assert!(
            message.contains("replicas"),
            "error names the field: {message}"
        );
    }

    /// A blocking mutex on a request path is the defect this module was
    /// restructured to remove: it parks an executor thread and poisons
    /// permanently on panic. Guarded here rather than left to review, because
    /// the failure it causes shows up as unrelated latency in other gears,
    /// far from the line that caused it.
    #[test]
    fn no_blocking_mutex_on_the_request_path() {
        // The blocking type's path, assembled at runtime so this test does not
        // trip over its own source text.
        let blocking_mutex = ["std", "sync", "Mutex"].join("::");

        for (name, source) in [
            ("session.rs", include_str!("session.rs")),
            ("tokenizer.rs", include_str!("tokenizer.rs")),
        ] {
            // Tests and prose are allowed to name it; only real code matters.
            let code = source.split("\n#[cfg(test)]").next().unwrap_or(source);
            let offending: Vec<&str> = code
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .filter(|line| line.contains(&blocking_mutex))
                .collect();

            assert!(
                offending.is_empty(),
                "{name} uses a blocking mutex on an inference path: {offending:?} — \
                 use tokio::sync::Mutex so the guard can move into spawn_blocking \
                 (see the module docs)"
            );
        }
    }
}

/// Concurrency, panic-recovery and cancel-safety tests, which need a session
/// and therefore a real ONNX model on disk. Enabled with
/// `--features real-model-tests`. The committed fixture graph that lets the
/// rest of the suite run without one is covered by
/// `docs/runtime/ADR/0007-cpt-cf-onnx-runner-adr-test-fixture-strategy.md`.
#[cfg(all(test, feature = "real-model-tests"))]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::use_debug)]
mod real_model_tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::LoadedModel;
    use crate::runtime::config::ModelConfig;

    /// The dtypes `gliner2-pii` actually emits. Mandatory now that an
    /// undeclared output is a load-time failure rather than a guess.
    fn gliner_output_dtypes()
    -> std::collections::BTreeMap<String, crate::runtime::config::OutputDtype> {
        use crate::runtime::config::OutputDtype;
        [
            ("span_probs".to_owned(), OutputDtype::F32),
            ("count_logits".to_owned(), OutputDtype::F32),
        ]
        .into_iter()
        .collect()
    }

    /// A model fetched by `scripts/fetch-models.sh`. The graph is never run
    /// here — these tests drive `with_replica` directly, so only loading has
    /// to succeed.
    fn test_model(replicas: usize) -> ModelConfig {
        let model_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("models/gliner2-pii")
            .canonicalize()
            .expect("run scripts/fetch-models.sh first");
        ModelConfig {
            model_id: "test-model".to_owned(),
            model_dir,
            model_file: "model.quantized.onnx".to_owned(),
            skip_checksum: true,
            expected_inputs: vec![
                "input_ids".to_owned(),
                "attention_mask".to_owned(),
                "text_word_indices".to_owned(),
                "schema_special_indices".to_owned(),
            ],
            expected_outputs: vec!["span_probs".to_owned(), "count_logits".to_owned()],
            output_dtypes: gliner_output_dtypes(),
            replicas,
            intra_threads: 1,
            ..ModelConfig::default()
        }
    }

    /// The test the old code could not pass: two replicas, a two-worker
    /// runtime, and two inferences that each occupy a thread for longer than
    /// the test's own timeout allows serially. Against the pre-offload code
    /// both closures ran inline on the two workers, leaving nothing to drive
    /// the timer, and this hung.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_calls_do_not_starve_the_runtime() {
        let model = Arc::new(
            LoadedModel::load(&test_model(2))
                .await
                .expect("model loads"),
        );

        let calls = (0..2).map(|_| {
            let model = Arc::clone(&model);
            tokio::spawn(async move {
                model
                    .with_replica(|_session| {
                        std::thread::sleep(Duration::from_millis(300));
                        Ok(())
                    })
                    .await
            })
        });

        let results = tokio::time::timeout(Duration::from_secs(5), async {
            let mut out = Vec::new();
            for call in calls {
                out.push(call.await.expect("task joins"));
            }
            out
        })
        .await
        .expect("both inferences complete without starving the runtime");

        assert!(results.iter().all(Result::is_ok), "both calls succeed");
    }

    /// An unrelated task must keep making progress while inference occupies a
    /// blocking thread — the property that makes this gear safe to deploy
    /// beside others in one process.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unrelated_tasks_progress_during_inference() {
        let model = Arc::new(
            LoadedModel::load(&test_model(1))
                .await
                .expect("model loads"),
        );
        let ticks = Arc::new(AtomicUsize::new(0));

        let ticker = {
            let ticks = Arc::clone(&ticks);
            tokio::spawn(async move {
                for _ in 0..10 {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    ticks.fetch_add(1, Ordering::Relaxed);
                }
            })
        };

        model
            .with_replica(|_session| {
                std::thread::sleep(Duration::from_millis(300));
                Ok(())
            })
            .await
            .expect("inference completes");

        assert!(
            ticks.load(Ordering::Relaxed) > 0,
            "an unrelated task made no progress while inference was in flight"
        );
        ticker.await.expect("ticker joins");
    }

    /// A panicking inference costs one session, not the gear: the call returns
    /// a typed error rather than unwinding into the caller, and the next call
    /// on the same model succeeds against a rebuilt session.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn panic_costs_one_session_and_the_next_call_recovers() {
        let model = LoadedModel::load(&test_model(1))
            .await
            .expect("model loads");

        let panicked = model
            .with_replica(|_session| -> Result<(), _> { panic!("simulated inference panic") })
            .await;
        assert!(panicked.is_err(), "a panicking inference returns an error");

        let recovered = model.with_replica(|_session| Ok(())).await;
        assert!(
            recovered.is_ok(),
            "the next call must rebuild the lost session: {recovered:?}"
        );
    }

    /// Cancelling a call must not strand a replica. The in-flight work runs to
    /// completion and the session returns to its slot, so the pool is intact
    /// afterwards.
    ///
    /// The cancellation has to happen *mid-flight* to prove anything: an async
    /// fn's future does nothing until first polled, so dropping one before
    /// awaiting it would exercise no code at all.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_call_leaves_the_replica_usable() {
        let model = LoadedModel::load(&test_model(1))
            .await
            .expect("model loads");

        for _ in 0..20 {
            let cancelled = tokio::time::timeout(
                Duration::from_millis(20),
                model.with_replica(|_session| {
                    std::thread::sleep(Duration::from_millis(200));
                    Ok(())
                }),
            )
            .await;
            assert!(
                cancelled.is_err(),
                "the call must be cancelled while in flight"
            );
        }

        let after = tokio::time::timeout(
            Duration::from_secs(30),
            model.with_replica(|_session| Ok(())),
        )
        .await
        .expect("a replica becomes available again after cancelled calls");
        assert!(
            after.is_ok(),
            "replica usable after cancellation: {after:?}"
        );
    }
}
