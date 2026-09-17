//! Plugin API trait implemented by scanner backend plugins.
//!
//! # Fan-out, not pick-one
//!
//! Unlike most plugin extension points in this platform (which select a
//! *single* plugin instance by vendor+priority via `choose_plugin_instance`),
//! `prompt-scanner` fans out to **every** scanner backend plugin instance
//! enabled for the calling tenant's policy, concurrently, with a per-backend
//! timeout. See `docs/DESIGN.md` and `domain::service::Service::resolve_backends`.
//!
//! Each scanner backend plugin registers:
//! 1. A `PluginV1<ScannerBackendSpecV1>` instance in `types-registry`
//!    (discoverable by the gateway).
//! 2. A scoped `ClientHub` entry for `dyn ScannerBackendClient`, keyed by
//!    the instance's GTS id (`ClientScope::gts_id`).

use async_trait::async_trait;
use toolkit_security::SecurityContext;
use uuid::Uuid;

use crate::errors::PromptScannerError;
use crate::models::{BackendOutcome, ScanInput, TenantPolicy};

/// Plugin API trait implemented by each scanner backend (DLP, secrets,
/// prompt injection, harmful content, malicious code/URL, ...).
///
/// # Security Context
///
/// The gateway passes the `SecurityContext` through without validation.
/// Backends that need to enforce their own access rules may do so, but
/// tenant isolation for *which* backends run is already enforced by the
/// gateway via `TenantPolicy.enabled_backends` before this trait is ever
/// invoked.
#[async_trait]
pub trait ScannerBackendClient: Send + Sync {
    /// Run this backend against the given input and return its outcome.
    ///
    /// Implementations should return quickly — the gateway applies a
    /// per-backend timeout (`Service::BACKEND_TIMEOUT` /
    /// `PromptScannerConfig::backend_timeout_ms`) and treats a slow backend
    /// the same as one that errored (degrades to "unscored" unless the
    /// tenant policy marks this backend `required`).
    ///
    /// Return [`BackendOutcome::Defer`] rather than blocking synchronously
    /// when the input exceeds this backend's own cost threshold (e.g. an
    /// oversized payload for an ONNX model) — see `docs/DESIGN.md`'s "Async
    /// deferral for oversized inputs".
    ///
    /// # Errors
    ///
    /// Implementations may return any [`PromptScannerError`]; the gateway
    /// treats all of them as "this backend did not produce a finding" for
    /// aggregation purposes (see `Service::aggregate_verdict`).
    async fn detect(
        &self,
        ctx: &SecurityContext,
        input: ScanInput,
    ) -> Result<BackendOutcome, PromptScannerError>;
}

/// Plugin API trait implemented by whatever currently owns tenant policy
/// storage/decisioning for `prompt-scanner`.
///
/// # Single-instance selection, not fan-out
///
/// Unlike [`ScannerBackendClient`], exactly one `PolicyProvider` is
/// authoritative at a time — there's no sound way to "aggregate" two
/// different tenant policies. `prompt-scanner` resolves the
/// highest-priority registered `PolicyProviderSpecV1` instance (see
/// `crate::gts::PolicyProviderSpecV1`), falling back to its own built-in
/// read-only provider, which serves one policy from the gear's config, when
/// no plugin instance is registered. prompt-scanner stores no tenant
/// policies itself. See `docs/POLICY_PROVIDER_AND_DEMO_CONSOLE.md`.
///
/// # Transport-agnostic
///
/// Deliberately does not take a `SecureORM` runner/scope — a provider backed
/// by an external policy service has no database connection to share.
/// Implementations are responsible for their own authorization if they need
/// it. `Service` only ever asks for the calling subject's own tenant on
/// `GET`/`PUT /policy`, and for the scanned tenant after authorizing the
/// scan.
#[async_trait]
pub trait PolicyProvider: Send + Sync {
    /// Fetch the policy configured for `tenant_id`, if any. `None` means
    /// "nothing configured for this tenant" — the caller falls back to
    /// [`TenantPolicy::default_for_tenant`].
    ///
    /// # Errors
    ///
    /// Returns [`PromptScannerError`] on authorization failure, or
    /// [`PromptScannerError::PolicyUnavailable`] when the provider's source
    /// is unreachable and it has no policy to serve for the tenant.
    async fn get_policy(
        &self,
        ctx: &SecurityContext,
        tenant_id: Uuid,
    ) -> Result<Option<TenantPolicy>, PromptScannerError>;

    /// Insert or replace the policy for `policy.tenant_id`, returning the
    /// stored value.
    ///
    /// # Errors
    ///
    /// Returns [`PromptScannerError`] on authorization failure, validation
    /// failure, or backend unavailability.
    async fn put_policy(
        &self,
        ctx: &SecurityContext,
        policy: TenantPolicy,
    ) -> Result<TenantPolicy, PromptScannerError>;
}
