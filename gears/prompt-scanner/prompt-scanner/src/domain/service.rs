//! Domain service for the prompt-scanner gear.
//!
//! Orchestrates the fan-out-to-all-enabled-backends scan flow described in
//! `docs/DESIGN.md`:
//!
//! 1. Resolve the tenant's [`TenantPolicy`] (`enabled_backends`).
//! 2. Query `types-registry` for **every** registered `ScannerBackendSpecV1`
//!    instance, filter by `enabled_backends`, and resolve each via
//!    `ClientHub::try_get_scoped::<dyn ScannerBackendClient>(&ClientScope::gts_id(...))`.
//! 3. Call every resolved backend **concurrently**, with a per-backend
//!    timeout. A backend that times out or errors degrades to "no
//!    finding" — unless the tenant policy marks it `required`, in which case
//!    the whole scan fails. A backend may instead return
//!    [`BackendOutcome::Defer`] to push its contribution onto the async job
//!    queue rather than blocking the synchronous scan — see "Async
//!    deferral" below.
//! 4. Aggregate findings into one verdict per `domain::policy::aggregate_verdict`.
//! 5. Persist a `ScanRecord` (the audit trail).
//!
//! This deliberately does **not** use `toolkit::plugins::choose_plugin_instance`
//! — that helper picks a single "best" instance by vendor+priority, which is
//! the wrong shape here (`prompt-scanner` wants "all enabled", not "one").
//!
//! # Async deferral for oversized inputs
//!
//! A backend that decides a given input is too expensive to evaluate
//! synchronously returns [`BackendOutcome::Defer`] instead of a
//! [`Finding`]. `run_backends` reacts by pushing a [`ScanJob::DeferredBackend`]
//! onto the same in-memory queue `scan_async` uses, tagged with the scan's
//! existing `trace_id` (not a new one) — the other, fast backends' findings
//! still aggregate into the verdict returned synchronously. The background
//! job worker (`process_next_job`) later calls just the deferring backend
//! and merges its finding into the existing `scan_record` row in place,
//! re-running `aggregate_verdict` across the original findings plus the new
//! one. See `gears/onnx-runner/docs/DESIGN.md`'s "Async deferral for
//! oversized inputs" for the full design rationale.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use chrono::Utc;
use file_parser_sdk::{Detection, FileParserClientV1, ParseBytesRequest};
use prompt_scanner_sdk::{
    BackendOutcome, ContentKind, Finding, PolicyProvider, PolicyProviderSpecV1, ScanInput,
    ScanJobHandle, ScanJobStatus, ScanRequest, ScanVerdict, ScannerBackendClient,
    ScannerBackendSpecV1, TenantPolicy, VerdictAction,
};
use tokio::sync::Mutex;
use toolkit::client_hub::{ClientHub, ClientScope};
use toolkit::gts::PluginV1;
use toolkit_db::DBProvider;
use toolkit_macros::domain_model;
use toolkit_odata::{ODataQuery, Page};
use toolkit_security::{AccessScope, SecurityContext, pep_properties};
use tracing::instrument;
use types_registry_sdk::{InstanceQuery, TypesRegistryClient};
use uuid::Uuid;

use authz_resolver_sdk::PolicyEnforcer;
use authz_resolver_sdk::pep::{AccessRequest, ResourceType};

use crate::domain::error::DomainError;
use crate::domain::policy::aggregate_verdict;
use crate::domain::repos::{ScanRecord, ScanRecordRepository};

pub(crate) type DbProvider = DBProvider<toolkit_db::DbError>;

/// Authorization resource types and their PEP-supported properties.
pub(crate) mod resources {
    use super::ResourceType;
    use toolkit_security::pep_properties;

    pub const SCAN_RECORD: ResourceType = ResourceType::from_static(
        "prompt_scanner.scan_record",
        &[pep_properties::OWNER_TENANT_ID, pep_properties::RESOURCE_ID],
    );
}

pub(crate) mod actions {
    pub const GET: &str = "get";
    pub const LIST: &str = "list";
    pub const CREATE: &str = "create";
    pub const UPDATE: &str = "update";
}

/// Default per-backend call timeout, used when the gear config doesn't
/// override it.
pub const DEFAULT_BACKEND_TIMEOUT: Duration = Duration::from_secs(5);

/// A queued asynchronous job, processed by the gear's background worker
/// (`Self::process_next_job`). Module-private: never crosses a layer
/// boundary, so it's exempt from `#[domain_model]`.
enum ScanJob {
    /// An explicit `scan_async` submission — run the full scan under a
    /// pre-allocated `trace_id`.
    Scan {
        job_id: Uuid,
        trace_id: Uuid,
        ctx: SecurityContext,
        request: ScanRequest,
    },
    /// A single backend's deferred contribution to an *already-recorded*
    /// scan — see the "Async deferral" module docs above. Not tracked in
    /// `job_status` (there's no external job handle for it; the caller
    /// already has the synchronous verdict and polls `get_verdict` for
    /// updates).
    DeferredBackend {
        trace_id: Uuid,
        ctx: SecurityContext,
        vendor: String,
        client: Arc<dyn ScannerBackendClient>,
        input: ScanInput,
    },
}

/// A registered scanner backend plugin instance, for `GET
/// /prompt-scanner/v1/backends`.
#[domain_model]
#[derive(Debug, Clone)]
pub struct BackendInstanceInfo {
    pub gts_id: String,
    pub vendor: String,
    pub priority: i16,
    /// Whether a scoped `ClientHub` entry is currently registered — i.e.
    /// whether the gateway could actually call this instance right now.
    pub available: bool,
}

/// Domain service for prompt-scanner.
#[domain_model]
pub struct Service<SR>
where
    SR: ScanRecordRepository + 'static,
{
    db: Arc<DbProvider>,
    hub: Arc<ClientHub>,
    scan_repo: Arc<SR>,
    /// Fallback [`PolicyProvider`] used when no `PolicyProviderSpecV1`
    /// plugin instance is registered — see `resolve_policy_provider`. Always
    /// the DB-backed provider in practice today, but kept as `Arc<dyn
    /// PolicyProvider>` (rather than a generic parameter) so `Service`
    /// itself never needs to know that.
    default_policy_provider: Arc<dyn PolicyProvider>,
    policy_enforcer: PolicyEnforcer,
    backend_timeout: Duration,
    /// In-memory FIFO queue for `scan_async` jobs and deferred backend
    /// contributions, drained by the gear's stateful background task (see
    /// `gear::PromptScanner::serve`).
    ///
    /// Scaffold-level durability: jobs are lost on process restart. A
    /// production implementation would back this with a durable queue
    /// (outbox table, message broker, ...) — deliberately deferred here.
    jobs: Mutex<VecDeque<ScanJob>>,
    job_status: Mutex<HashMap<Uuid, ScanJobStatus>>,
}

impl<SR> Service<SR>
where
    SR: ScanRecordRepository + 'static,
{
    #[must_use]
    pub fn new(
        db: Arc<DbProvider>,
        hub: Arc<ClientHub>,
        scan_repo: Arc<SR>,
        default_policy_provider: Arc<dyn PolicyProvider>,
        policy_enforcer: PolicyEnforcer,
        backend_timeout: Duration,
    ) -> Self {
        Self {
            db,
            hub,
            scan_repo,
            default_policy_provider,
            policy_enforcer,
            backend_timeout,
            jobs: Mutex::new(VecDeque::new()),
            job_status: Mutex::new(HashMap::new()),
        }
    }

    /// Resolve the currently authoritative [`PolicyProvider`]: the
    /// highest-priority registered `PolicyProviderSpecV1` plugin instance,
    /// or `default_policy_provider` if none is registered. See
    /// `prompt_scanner_sdk::PolicyProvider`'s docs for why this is a
    /// single-instance pick, unlike `resolve_backends`' fan-out.
    async fn resolve_policy_provider(&self) -> Result<Arc<dyn PolicyProvider>, DomainError> {
        let registry = self.hub.get::<dyn TypesRegistryClient>()?;
        let type_id = PolicyProviderSpecV1::gts_type_id().clone();

        let instances = registry
            .list_instances(InstanceQuery::new().with_pattern(format!("{type_id}*")))
            .await?;

        let mut best: Option<(String, i16)> = None;
        for inst in &instances {
            let plugin: PluginV1<PolicyProviderSpecV1> =
                match serde_json::from_value(inst.object.clone()) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!(
                            gts_id = %inst.id,
                            error = %e,
                            "failed to deserialize policy provider plugin instance"
                        );
                        continue;
                    }
                };

            let is_better = best
                .as_ref()
                .is_none_or(|(_, priority)| plugin.priority < *priority);
            if is_better {
                best = Some((inst.id.to_string(), plugin.priority));
            }
        }

        let Some((gts_id, _)) = best else {
            return Ok(Arc::clone(&self.default_policy_provider));
        };

        if let Some(provider) = self
            .hub
            .try_get_scoped::<dyn PolicyProvider>(&ClientScope::gts_id(&gts_id))
        {
            return Ok(provider);
        }

        tracing::warn!(
            gts_id = %gts_id,
            "policy provider plugin instance registered but client not found; \
             falling back to the default provider"
        );
        Ok(Arc::clone(&self.default_policy_provider))
    }

    // ── Fan-out backend resolution ───────────────────────────────────

    /// Resolve every registered `ScannerBackendSpecV1` instance whose
    /// `vendor` is present in `enabled`, returning `(vendor, client)` pairs.
    ///
    /// Instances that fail to deserialize, or whose scoped `ClientHub` entry
    /// isn't registered yet, are skipped with a warning rather than failing
    /// the whole resolution — callers decide whether a missing *required*
    /// backend should fail the scan (see `run_backends`).
    async fn resolve_backends(
        &self,
        enabled: &[String],
    ) -> Result<Vec<(String, Arc<dyn ScannerBackendClient>)>, DomainError> {
        if enabled.is_empty() {
            return Ok(Vec::new());
        }

        let registry = self.hub.get::<dyn TypesRegistryClient>()?;
        let type_id = ScannerBackendSpecV1::gts_type_id().clone();

        let instances = registry
            .list_instances(InstanceQuery::new().with_pattern(format!("{type_id}*")))
            .await?;

        let mut resolved = Vec::new();
        for inst in &instances {
            let plugin: PluginV1<ScannerBackendSpecV1> =
                match serde_json::from_value(inst.object.clone()) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!(
                            gts_id = %inst.id,
                            error = %e,
                            "failed to deserialize scanner backend plugin instance"
                        );
                        continue;
                    }
                };

            if !enabled.iter().any(|v| v == &plugin.vendor) {
                continue;
            }

            let scope = ClientScope::gts_id(&inst.id);
            match self.hub.try_get_scoped::<dyn ScannerBackendClient>(&scope) {
                Some(client) => resolved.push((plugin.vendor.clone(), client)),
                None => {
                    tracing::warn!(
                        vendor = %plugin.vendor,
                        gts_id = %inst.id,
                        "scanner backend client not registered yet"
                    );
                }
            }
        }

        Ok(resolved)
    }

    /// Fan out to every resolved backend concurrently (with a per-backend
    /// timeout) and return the findings that came back synchronously.
    ///
    /// A required backend (per `policy.required_backends`) that is
    /// unresolvable, errors, or times out fails the whole call with
    /// `RequiredBackendFailed` / `BackendUnavailable`. A non-required
    /// backend in the same situation is simply omitted from the result
    /// ("unscored"), per `docs/DESIGN.md`. A backend that returns
    /// [`BackendOutcome::Defer`] (required or not) is queued for background
    /// processing under `trace_id` rather than counted as a failure — see
    /// the module-level "Async deferral" docs above.
    #[allow(clippy::cognitive_complexity)]
    async fn run_backends(
        &self,
        ctx: &SecurityContext,
        req: &ScanRequest,
        policy: &TenantPolicy,
        trace_id: Uuid,
    ) -> Result<Vec<Finding>, DomainError> {
        let resolved = self.resolve_backends(&policy.enabled_backends).await?;

        for required in &policy.required_backends {
            if !resolved.iter().any(|(v, _)| v == required) {
                return Err(DomainError::BackendUnavailable {
                    backend: required.clone(),
                    reason: "no resolvable plugin instance for this vendor".to_owned(),
                });
            }
        }

        let input = req.to_scan_input();
        let timeout = self.backend_timeout;

        let futs = resolved.iter().map(|(vendor, client)| {
            let input = input.clone();
            let client = Arc::clone(client);
            async move {
                let outcome = tokio::time::timeout(timeout, client.detect(ctx, input)).await;
                (vendor.clone(), client, outcome)
            }
        });

        let outcomes = futures::future::join_all(futs).await;

        let mut findings = Vec::with_capacity(outcomes.len());
        for (vendor, client, outcome) in outcomes {
            let required = policy.required_backends.iter().any(|d| d == &vendor);
            match outcome {
                Ok(Ok(BackendOutcome::Findings(mut backend_findings))) => {
                    for finding in &mut backend_findings {
                        if finding.detector.is_empty() {
                            finding.detector.clone_from(&vendor);
                        }
                    }
                    findings.extend(backend_findings);
                }
                Ok(Ok(BackendOutcome::Defer)) => {
                    tracing::info!(
                        backend = %vendor,
                        trace_id = %trace_id,
                        "backend deferred; queuing for background processing"
                    );
                    self.jobs.lock().await.push_back(ScanJob::DeferredBackend {
                        trace_id,
                        ctx: ctx.clone(),
                        vendor,
                        client,
                        input: input.clone(),
                    });
                }
                Ok(Err(e)) => {
                    tracing::warn!(backend = %vendor, error = %e, "backend returned an error");
                    if required {
                        return Err(DomainError::RequiredBackendFailed {
                            backend: vendor,
                            reason: e.to_string(),
                        });
                    }
                }
                Err(_elapsed) => {
                    tracing::warn!(backend = %vendor, "backend timed out");
                    if required {
                        return Err(DomainError::RequiredBackendFailed {
                            backend: vendor,
                            reason: "timed out".to_owned(),
                        });
                    }
                }
            }
        }

        Ok(findings)
    }

    /// List every registered scanner backend plugin instance, regardless of
    /// tenant policy — used by `GET /prompt-scanner/v1/backends` for
    /// operator/admin visibility (not part of the fan-out authorization
    /// path, which filters by `TenantPolicy.enabled_backends` instead).
    pub async fn list_backend_instances(&self) -> Result<Vec<BackendInstanceInfo>, DomainError> {
        let registry = self.hub.get::<dyn TypesRegistryClient>()?;
        let type_id = ScannerBackendSpecV1::gts_type_id().clone();

        let instances = registry
            .list_instances(InstanceQuery::new().with_pattern(format!("{type_id}*")))
            .await?;

        let mut out = Vec::with_capacity(instances.len());
        for inst in &instances {
            let plugin: PluginV1<ScannerBackendSpecV1> = match serde_json::from_value(
                inst.object.clone(),
            ) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(gts_id = %inst.id, error = %e, "failed to deserialize scanner backend plugin instance");
                    continue;
                }
            };
            let scope = ClientScope::gts_id(&inst.id);
            let available = self
                .hub
                .try_get_scoped::<dyn ScannerBackendClient>(&scope)
                .is_some();
            out.push(BackendInstanceInfo {
                gts_id: inst.id.to_string(),
                vendor: plugin.vendor,
                priority: plugin.priority,
                available,
            });
        }
        Ok(out)
    }

    // ── Business logic ───────────────────────────────────────────────

    /// If `req.content_kind == ContentKind::Upload` carries an `attachment`,
    /// decode it and replace `req.payload` with the text `file-parser`
    /// extracts from it, so the rest of the pipeline (backend fan-out,
    /// redaction, audit trail) only ever deals with text — exactly as it
    /// did before attachments existed.
    ///
    /// This is what lets a connector (`forward-proxy`'s interceptor plugin,
    /// `llm-gateway`'s `scan-hook-plugin`, an endpoint agent, ...) send raw
    /// attachment bytes straight to `prompt-scanner` in a single call, instead
    /// of calling `file-parser` itself first — see `docs/DESIGN.md`.
    async fn resolve_effective_payload(
        &self,
        ctx: &SecurityContext,
        mut req: ScanRequest,
    ) -> Result<ScanRequest, DomainError> {
        let Some(attachment) = req.attachment.take() else {
            return Ok(req);
        };

        if req.content_kind != ContentKind::Upload {
            tracing::warn!(
                content_kind = ?req.content_kind,
                "attachment present on a non-Upload scan request; ignoring it"
            );
            return Ok(req);
        }

        let bytes = base64::engine::general_purpose::STANDARD
            .decode(attachment.data_base64.as_bytes())
            .map_err(|e| DomainError::AttachmentParsingFailed {
                reason: format!("invalid base64: {e}"),
            })?;

        let client = self.hub.get::<dyn FileParserClientV1>()?;
        let parsed = client
            .parse_bytes(
                ctx,
                ParseBytesRequest {
                    filename: attachment.filename,
                    content_type: attachment.mime_type,
                    bytes: bytes::Bytes::from(bytes),
                    // A connector's declared MIME type is an untrusted
                    // hint — let content-based detection correct a wrong
                    // or missing one so scanning runs against the right
                    // backend rather than silently mis-parsing.
                    detection: Detection::Auto,
                },
            )
            .await?;

        req.payload = parsed.markdown;
        Ok(req)
    }

    fn validate_request(req: &ScanRequest) -> Result<(), DomainError> {
        if req.payload.is_empty() {
            return Err(DomainError::validation(
                "payload",
                "payload must not be empty",
            ));
        }
        if req.connector_id.trim().is_empty() {
            return Err(DomainError::validation(
                "connector_id",
                "connector_id must not be empty",
            ));
        }
        Ok(())
    }

    pub(crate) fn validate_policy(policy: &TenantPolicy) -> Result<(), DomainError> {
        for (category, threshold) in &policy.thresholds {
            if !(0.0..=1.0).contains(threshold) {
                return Err(DomainError::validation(
                    "thresholds",
                    format!("threshold for '{category}' must be in [0.0, 1.0]"),
                ));
            }
        }
        for required in &policy.required_backends {
            if !policy.enabled_backends.contains(required) {
                return Err(DomainError::validation(
                    "required_backends",
                    format!("'{required}' must also be listed in enabled_backends"),
                ));
            }
        }
        Ok(())
    }

    /// Synchronous scan: run backends, aggregate, persist, return verdict.
    #[instrument(skip(self, ctx, req), fields(tenant_id = %req.tenant_id, connector_id = %req.connector_id))]
    pub async fn scan(
        &self,
        ctx: &SecurityContext,
        req: ScanRequest,
    ) -> Result<ScanVerdict, DomainError> {
        self.scan_with_trace_id(ctx, req, Uuid::now_v7()).await
    }

    async fn scan_with_trace_id(
        &self,
        ctx: &SecurityContext,
        req: ScanRequest,
        trace_id: Uuid,
    ) -> Result<ScanVerdict, DomainError> {
        let req = self.resolve_effective_payload(ctx, req).await?;
        Self::validate_request(&req)?;

        let conn = self.db.conn().map_err(DomainError::from)?;

        // CREATE authorization: can this subject submit scans for req.tenant_id?
        let scope = self
            .policy_enforcer
            .access_scope_with(
                ctx,
                &resources::SCAN_RECORD,
                actions::CREATE,
                None,
                &AccessRequest::new()
                    .resource_property(pep_properties::OWNER_TENANT_ID, req.tenant_id),
            )
            .await?;

        let policy = self.load_policy(ctx, req.tenant_id).await?;
        let (action, considered) = if policy.content_kinds.contains(&req.content_kind) {
            let findings = self.run_backends(ctx, &req, &policy, trace_id).await?;
            aggregate_verdict(&findings, &policy)
        } else {
            // Not this policy's business: no backends, allowed, still recorded.
            tracing::debug!(content_kind = ?req.content_kind, "content kind not governed by the tenant policy");
            (VerdictAction::Allow, Vec::new())
        };

        let redacted_payload = (action == VerdictAction::Redact)
            .then(|| prompt_scanner_sdk::redact_spans(&req.payload, &considered));

        let record = ScanRecord {
            trace_id,
            tenant_id: req.tenant_id,
            connector_id: req.connector_id,
            content_kind: req.content_kind,
            action,
            findings: considered,
            redacted_payload,
            created_at: Utc::now(),
        };

        let saved = self.scan_repo.create(&conn, &scope, record).await?;
        Ok(saved.into())
    }

    /// Queue a scan for asynchronous processing. The background worker
    /// (`Self::spawn_job_worker`) drains the queue and calls
    /// `scan_with_trace_id` under the hood, so the eventual verdict is
    /// available via `get_verdict(trace_id)`.
    #[instrument(skip(self, ctx, req), fields(tenant_id = %req.tenant_id))]
    pub async fn scan_async(
        &self,
        ctx: &SecurityContext,
        req: ScanRequest,
    ) -> Result<ScanJobHandle, DomainError> {
        Self::validate_request(&req)?;

        let job_id = Uuid::now_v7();
        let trace_id = Uuid::now_v7();

        self.job_status
            .lock()
            .await
            .insert(job_id, ScanJobStatus::Queued);
        self.jobs.lock().await.push_back(ScanJob::Scan {
            job_id,
            trace_id,
            ctx: ctx.clone(),
            request: req,
        });

        Ok(ScanJobHandle {
            job_id,
            trace_id,
            status: ScanJobStatus::Queued,
        })
    }

    /// Current status of a previously-queued async job, if known.
    ///
    /// Not currently exposed over REST (the DESIGN.md endpoint table only
    /// lists `GET /scans/{trace_id}` for polling the eventual verdict) but
    /// useful for tests and future `GET /scans/async/{job_id}` support.
    pub async fn job_status(&self, job_id: Uuid) -> Option<ScanJobStatus> {
        self.job_status.lock().await.get(&job_id).copied()
    }

    /// Pop and process the next queued job, if any. Returns `true` if a job
    /// was processed. Called in a loop by the gear's background task.
    pub async fn process_next_job(&self) -> bool {
        let Some(job) = self.jobs.lock().await.pop_front() else {
            return false;
        };

        match job {
            ScanJob::Scan {
                job_id,
                trace_id,
                ctx,
                request,
            } => {
                self.job_status
                    .lock()
                    .await
                    .insert(job_id, ScanJobStatus::Running);

                let result = self.scan_with_trace_id(&ctx, request, trace_id).await;

                let status = match result {
                    Ok(_) => ScanJobStatus::Completed,
                    Err(e) => {
                        tracing::error!(job_id = %job_id, error = %e, "async scan job failed");
                        ScanJobStatus::Failed
                    }
                };
                self.job_status.lock().await.insert(job_id, status);
            }
            ScanJob::DeferredBackend {
                trace_id,
                ctx,
                vendor,
                client,
                input,
            } => {
                self.process_deferred_backend(trace_id, &ctx, &vendor, client.as_ref(), input)
                    .await;
            }
        }
        true
    }

    /// Call a previously-deferred backend and merge its outcome into the
    /// already-recorded `scan_record` for `trace_id`. Errors are logged,
    /// not propagated — there's no caller waiting on this background call
    /// (see the module-level "Async deferral" docs).
    async fn process_deferred_backend(
        &self,
        trace_id: Uuid,
        ctx: &SecurityContext,
        vendor: &str,
        client: &dyn ScannerBackendClient,
        input: ScanInput,
    ) {
        // Captured before `input` is moved into `detect`: this is the
        // authoritative tenant for the merge, so it never has to be recovered
        // from an unscoped read of the record.
        let tenant_id = input.tenant_id;
        let outcome = client.detect(ctx, input).await;
        let findings = Self::deferred_outcome_to_findings(trace_id, vendor, outcome);
        if findings.is_empty() {
            return;
        }

        if let Err(e) = self
            .merge_deferred_findings(ctx, tenant_id, trace_id, findings)
            .await
        {
            tracing::error!(
                trace_id = %trace_id,
                backend = %vendor,
                error = %e,
                "failed to merge deferred findings into scan record"
            );
        }
    }

    /// Extracts usable [`Finding`]s from a deferred backend's outcome,
    /// logging and returning an empty vec for the two non-finding cases (a
    /// repeat `Defer`, or an error).
    fn deferred_outcome_to_findings(
        trace_id: Uuid,
        vendor: &str,
        outcome: Result<BackendOutcome, prompt_scanner_sdk::PromptScannerError>,
    ) -> Vec<Finding> {
        match outcome {
            Ok(BackendOutcome::Findings(mut findings)) => {
                for f in &mut findings {
                    if f.detector.is_empty() {
                        vendor.clone_into(&mut f.detector);
                    }
                }
                findings
            }
            Ok(BackendOutcome::Defer) => {
                tracing::warn!(
                    trace_id = %trace_id,
                    backend = %vendor,
                    "deferred backend deferred again; dropping rather than re-queuing indefinitely"
                );
                Vec::new()
            }
            Err(e) => {
                tracing::warn!(
                    trace_id = %trace_id,
                    backend = %vendor,
                    error = %e,
                    "deferred backend failed"
                );
                Vec::new()
            }
        }
    }

    /// Append `new_findings` to the `scan_record` for `trace_id`, re-run
    /// `aggregate_verdict` across the combined findings, and persist the
    /// updated action/findings in place.
    async fn merge_deferred_findings(
        &self,
        ctx: &SecurityContext,
        tenant_id: Uuid,
        trace_id: Uuid,
        new_findings: Vec<Finding>,
    ) -> Result<(), DomainError> {
        let conn = self.db.conn().map_err(DomainError::from)?;

        // `tenant_id` comes from the queued job, not from reading the row: the
        // scan that enqueued this deferral was already authorized for CREATE
        // against that tenant. That removes the prefetch entirely — unlike
        // `get_verdict`, nothing here has to read a row to discover whose it is,
        // so there is no `AccessScope::allow_all()` and no foreign row ever
        // enters this process. Feeding the PDP a tenant taken from an unscoped
        // read would have meant asking about whatever tenant the row claimed,
        // rather than the one the deferring caller is entitled to.
        //
        // Constraints are required here (no `require_constraints(false)`),
        // matching `scan`'s CREATE check: this is a write, and the rest of the
        // gear's write path already refuses an allow that carries none.
        let scope = self
            .policy_enforcer
            .access_scope_with(
                ctx,
                &resources::SCAN_RECORD,
                actions::UPDATE,
                Some(trace_id),
                &AccessRequest::new().resource_property(pep_properties::OWNER_TENANT_ID, tenant_id),
            )
            .await?;

        let record = self
            .scan_repo
            .get(&conn, &scope, trace_id)
            .await?
            .ok_or_else(|| DomainError::verdict_not_found(trace_id))?;

        let policy = self.load_policy(ctx, tenant_id).await?;

        let mut findings = record.findings;
        findings.extend(new_findings);
        let (action, considered) = aggregate_verdict(&findings, &policy);

        self.scan_repo
            .update_findings(&conn, &scope, trace_id, action, considered)
            .await?;
        Ok(())
    }

    /// Fetch a previously recorded verdict by trace id.
    #[instrument(skip(self, ctx), fields(trace_id = %trace_id))]
    pub async fn get_verdict(
        &self,
        ctx: &SecurityContext,
        trace_id: Uuid,
    ) -> Result<ScanVerdict, DomainError> {
        let conn = self.db.conn().map_err(DomainError::from)?;

        // The PDP decision needs the record's `owner_tenant_id`, which cannot
        // be known without reading the row — so the read happens twice, and the
        // first one is scoped to the caller's own tenant. It must not be
        // `AccessScope::allow_all()`: `scan_records` is tenant-scoped
        // (`tenant_col = tenant_id`), so an unscoped prefetch pulls another
        // tenant's findings and `redacted_payload` into this process before any
        // authorization has happened. Same shape as `file-storage`'s
        // `get_file` (`domain/service/read_ops.rs`): scoped prefetch, decide,
        // re-read under the decision.
        let prefetch_scope = AccessScope::for_tenant(ctx.subject_tenant_id());
        let record = self
            .scan_repo
            .get(&conn, &prefetch_scope, trace_id)
            .await?
            .ok_or_else(|| DomainError::verdict_not_found(trace_id))?;

        let scope = self
            .policy_enforcer
            .access_scope_with(
                ctx,
                &resources::SCAN_RECORD,
                actions::GET,
                Some(trace_id),
                &AccessRequest::new()
                    .resource_property(pep_properties::OWNER_TENANT_ID, record.tenant_id)
                    .require_constraints(false),
            )
            .await?;

        // Re-read unconditionally. Short-circuiting on
        // `scope.is_unconstrained()` would return a row that only the prefetch
        // scope ever authorized, so the record handed back is always the one
        // the PDP-derived scope permits.
        let record = self
            .scan_repo
            .get(&conn, &scope, trace_id)
            .await?
            .ok_or_else(|| DomainError::verdict_not_found(trace_id))?;

        Ok(record.into())
    }

    /// `OData`-filterable audit search over recorded verdicts.
    #[instrument(skip(self, ctx, query))]
    pub async fn list_scans(
        &self,
        ctx: &SecurityContext,
        query: &ODataQuery,
    ) -> Result<Page<ScanVerdict>, DomainError> {
        let conn = self.db.conn().map_err(DomainError::from)?;

        let scope = self
            .policy_enforcer
            .access_scope(ctx, &resources::SCAN_RECORD, actions::LIST, None)
            .await?;

        let page = self.scan_repo.list_page(&conn, &scope, query).await?;
        Ok(page.map_items(Into::into))
    }

    /// Resolve the policy for `tenant_id` via the currently authoritative
    /// [`PolicyProvider`] (see `resolve_policy_provider`), falling back to
    /// a permissive default if the provider has nothing configured.
    async fn load_policy(
        &self,
        ctx: &SecurityContext,
        tenant_id: Uuid,
    ) -> Result<TenantPolicy, DomainError> {
        let provider = self.resolve_policy_provider().await?;
        let policy = provider.get_policy(ctx, tenant_id).await?;
        Ok(policy.unwrap_or_else(|| TenantPolicy::default_for_tenant(tenant_id)))
    }

    /// Get the calling subject's tenant policy (falls back to a permissive
    /// default if none has been configured yet).
    #[instrument(skip(self, ctx))]
    pub async fn get_policy(&self, ctx: &SecurityContext) -> Result<TenantPolicy, DomainError> {
        self.load_policy(ctx, ctx.subject_tenant_id()).await
    }

    /// Replace the calling subject's tenant policy, via the currently
    /// authoritative [`PolicyProvider`].
    #[instrument(skip(self, ctx, policy))]
    pub async fn put_policy(
        &self,
        ctx: &SecurityContext,
        mut policy: TenantPolicy,
    ) -> Result<TenantPolicy, DomainError> {
        policy.tenant_id = ctx.subject_tenant_id();
        Self::validate_policy(&policy)?;

        let provider = self.resolve_policy_provider().await?;
        Ok(provider.put_policy(ctx, policy).await?)
    }
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod service_tests;
