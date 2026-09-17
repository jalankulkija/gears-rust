//! Domain service for `demo-policy-console`.
//!
//! Holds one in-memory toggle (`DemoPolicyState`) and exposes it two ways:
//! - as a [`PolicyProvider`] plugin instance `prompt-scanner` discovers and
//!   resolves (see `gear.rs`'s registration and
//!   `docs/POLICY_PROVIDER_AND_DEMO_CONSOLE.md`);
//! - via this gear's own REST API (`api::rest`), which is what the demo web
//!   UI actually reads/writes.
//!
//! Scaffold-level durability, same tradeoff `prompt-scanner`'s own in-memory
//! job queue makes: state is lost on process restart. This whole gear is
//! explicitly temporary — see the module docs above and the design doc.

use std::sync::Arc;

use async_trait::async_trait;
use prompt_scanner_sdk::{
    PolicyProvider, PromptScannerClientV1, PromptScannerError, ScanVerdict, TenantPolicy,
    VerdictAction,
};
use tokio::sync::RwLock;
use toolkit::client_hub::ClientHub;
use toolkit_macros::domain_model;
use toolkit_odata::{ODataQuery, Page};
use toolkit_security::SecurityContext;
use uuid::Uuid;

/// Category prefix the demo's toggle + radio buttons control. Both
/// PII-producing scanner backends (`regex-scanner`, `onnx-scanner`) emit
/// categories under this prefix — see
/// `docs/POLICY_PROVIDER_AND_DEMO_CONSOLE.md`.
pub const PII_CATEGORY_PREFIX: &str = "pii";

/// Every category-prefix family a scanner backend in this demo pipeline
/// produces — see `regex-scanner`'s `patterns/{contact,financial,secrets}.rs`
/// and `onnx-scanner`'s `category_prefix` config. The toggle/radio in the
/// web UI is labeled "PII detection", but a demo user reasonably expects it
/// to govern *everything* the pipeline flags, not just categories literally
/// prefixed `"pii."` — `regex-scanner`'s credit-card/IBAN/secret detectors
/// hardcode their own `suggested_action` (e.g. `Block` for credit cards,
/// unconditionally) precisely because those categories used to fall outside
/// any override this gear applied, so the toggle silently had no effect on
/// them. Overriding all three prefixes closes that gap.
const OVERRIDE_CATEGORY_PREFIXES: &[&str] = &[PII_CATEGORY_PREFIX, "financial", "secret"];

/// The demo console's whole state: whether PII detection has any effect on
/// the verdict, and which action it maps to when it does.
///
/// `enabled_backends` isn't exposed on the web UI (there's no toggle for
/// it) — it's config-seeded (`DemoPolicyConsoleConfig::enabled_backends`)
/// and just carried along so `effective_policy` can build a complete,
/// working `TenantPolicy` on its own now that this gear is
/// `prompt-scanner`'s resolved provider. See
/// `docs/POLICY_PROVIDER_AND_DEMO_CONSOLE.md`.
#[domain_model]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DemoPolicyState {
    pub pii_enabled: bool,
    pub pii_action: VerdictAction,
    pub enabled_backends: Vec<String>,
}

/// Domain service for `demo-policy-console`.
#[domain_model]
pub struct Service {
    state: RwLock<DemoPolicyState>,
    /// Used only to read the audit log via `PromptScannerClientV1::list_scans`
    /// for the UI's audit panel — this gear never calls `scan`/`put_policy`
    /// on it.
    hub: Arc<ClientHub>,
}

impl Service {
    #[must_use]
    pub fn new(initial: DemoPolicyState, hub: Arc<ClientHub>) -> Self {
        Self {
            state: RwLock::new(initial),
            hub,
        }
    }

    pub async fn state(&self) -> DemoPolicyState {
        self.state.read().await.clone()
    }

    /// Updates the toggle/response-action, preserving the config-seeded
    /// `enabled_backends` (the web UI only ever sends `pii_enabled` +
    /// `pii_action` — see `api::rest::dto::DemoStateDto`).
    pub async fn set_state(&self, pii_enabled: bool, pii_action: VerdictAction) -> DemoPolicyState {
        let mut guard = self.state.write().await;
        guard.pii_enabled = pii_enabled;
        guard.pii_action = pii_action;
        guard.clone()
    }

    /// Builds the `TenantPolicy` the demo's current toggle state implies —
    /// see the [`PolicyProvider`] impl below. Same policy for every tenant:
    /// this is a single global demo toggle, not real per-tenant config.
    async fn effective_policy(&self, tenant_id: Uuid) -> TenantPolicy {
        let state = self.state().await;
        let mut policy = TenantPolicy::default_for_tenant(tenant_id);
        policy.enabled_backends.clone_from(&state.enabled_backends);
        let action = if state.pii_enabled {
            state.pii_action
        } else {
            // "Detection off" reads as "detection never changes the
            // verdict" — PII findings are still produced and recorded for
            // audit, they just can't move the aggregated action. See
            // docs/POLICY_PROVIDER_AND_DEMO_CONSOLE.md.
            VerdictAction::Allow
        };
        for prefix in OVERRIDE_CATEGORY_PREFIXES {
            policy
                .category_action_overrides
                .insert((*prefix).to_owned(), action);
        }
        policy
    }

    /// Thin passthrough to `PromptScannerClientV1::list_scans`, for the
    /// audit log panel.
    ///
    /// # Errors
    ///
    /// Returns [`PromptScannerError`] if `prompt-scanner` isn't registered in
    /// `ClientHub` or the underlying call fails.
    pub async fn list_scans(
        &self,
        ctx: &SecurityContext,
        query: &ODataQuery,
    ) -> Result<Page<ScanVerdict>, PromptScannerError> {
        let client = self
            .hub
            .get::<dyn PromptScannerClientV1>()
            .map_err(|e| PromptScannerError::internal(e.to_string()))?;
        client.list_scans(ctx, query).await
    }
}

#[async_trait]
impl PolicyProvider for Service {
    async fn get_policy(
        &self,
        _ctx: &SecurityContext,
        tenant_id: Uuid,
    ) -> Result<Option<TenantPolicy>, PromptScannerError> {
        Ok(Some(self.effective_policy(tenant_id).await))
    }

    /// Not meaningfully supported — the demo console is driven by its own
    /// `/state` endpoint (see `api::rest`), not `prompt-scanner`'s
    /// `PUT /policy`. Echoes back the currently effective policy rather than
    /// erroring, so a caller unaware of the distinction still gets a
    /// sensible, consistent response instead of a surprising failure.
    async fn put_policy(
        &self,
        ctx: &SecurityContext,
        policy: TenantPolicy,
    ) -> Result<TenantPolicy, PromptScannerError> {
        Ok(self
            .get_policy(ctx, policy.tenant_id)
            .await?
            .unwrap_or(policy))
    }
}
