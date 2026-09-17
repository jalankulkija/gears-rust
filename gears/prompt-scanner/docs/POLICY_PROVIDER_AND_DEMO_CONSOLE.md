# `PolicyProvider` trait + PII demo console — scope sketch

Companion to `docs/DESIGN.md`. Covers two related pieces of work:

1. Turning today's hard-wired, DB-only tenant policy lookup into a
   `PolicyProvider` extension point, so a future external policy-provider adapter
   can replace it without touching `Service`.
2. A small, explicitly **temporary/demo** gear that plugs into that
   extension point to give the GenAI Security demo a web UI: an audit log
   view, a PII-detection on/off toggle, and a set of radio buttons to choose
   the response action (`Block` / `Redact` / `Warn` / `Audit` / `Allow`).

**Status: implemented.** Both pieces below have been built, wired into
`config/genai-security.yaml`'s pipeline (behind the `demo-policy-console`
Cargo feature), and exercised end-to-end (`GET`/`PUT /demo-policy-console/v1/state`,
the audit passthrough, and a real scan whose verdict flips between `redact`
and `allow` as the toggle changes). Sections below are kept close to the
original design sketch, with "as implemented" notes where the actual shape
differs from the initial plan.

**Since then:** the DB-backed default provider is gone, along with the
`tenant_policies` table, `TenantPolicyRepository` and
`OrmTenantPolicyRepository`. The built-in fallback is now
`ConfigPolicyProvider`: one read-only policy from the gear config's
`fallback_policy`, or `None` when unset. Mentions of the DB path below are
history.

## 1. Current state (as of this branch)

- `TenantPolicy` (`prompt-scanner-sdk/src/models.rs`) is the per-tenant config:
  `enabled_backends`, `required_backends`, `thresholds`, `default_action`.
- `TenantPolicyRepository` (`prompt-scanner/src/domain/repos.rs`) is a
  DB-only trait (`get`/`upsert` against `SecureORM`). `Service::load_policy`
  calls it directly, after a PEP authorization check
  (`policy_enforcer.access_scope_with(..., TENANT_POLICY, GET/UPDATE, ...)`),
  falling back to `TenantPolicy::default_for_tenant` (permissive: no backends
  enabled, `Allow`) when nothing is configured.
- There is **no plugin extension point for policy** — unlike scanner
  backends, which are discovered via a `ScannerBackendSpecV1` GTS type and
  fanned out to over `ClientHub`, policy resolution is a single hard-coded
  path: PEP check → `OrmTenantPolicyRepository` → Postgres/SQLite.
- `aggregate_verdict` (`domain/policy.rs`) takes the **max** `VerdictAction`
  across findings that pass their category's confidence threshold; each
  finding's `suggested_action` is fixed by the detector that produced it
  (e.g. `regex-scanner`'s `ContactRecognizer` always suggests `Redact` for
  `pii.email`/`pii.url`/... — see `plugins/regex-scanner/src/domain/patterns/contact.rs`).
  Nothing in `TenantPolicy` today can override a detector's own suggested
  action per category — policy can only drop low-confidence findings or set
  the *no-findings* fallback (`default_action`).
- Both PII-producing backends (`regex-scanner`'s `ContactRecognizer` and
  `onnx-scanner`'s MiniLM-based classifier) emit categories prefixed
  `"pii."` (`pii.email`, `pii.url`, `pii.ip_address`, `pii.phone`, ...).
  Toggling "PII detection" and its response mode therefore means acting on
  the `"pii."` category prefix, not on a single backend vendor.

## 2. `PolicyProvider`: shape of the trait

Mirror the existing scanner-backend plugin pattern (`ScannerBackendClient` +
`ScannerBackendSpecV1`, both in `prompt-scanner-sdk`) rather than inventing a
new mechanism:

- **New trait**, `prompt-scanner-sdk/src/plugin_api.rs` (alongside
  `ScannerBackendClient`):

  ```rust
  #[async_trait]
  pub trait PolicyProvider: Send + Sync {
      async fn get_policy(
          &self,
          ctx: &SecurityContext,
          tenant_id: Uuid,
      ) -> Result<Option<TenantPolicy>, PromptScannerError>;

      async fn put_policy(
          &self,
          ctx: &SecurityContext,
          policy: TenantPolicy,
      ) -> Result<TenantPolicy, PromptScannerError>;
  }
  ```

  Transport-agnostic and `SecureORM`-free (unlike `TenantPolicyRepository`)
  so an HTTP-backed external policy-provider adapter can implement it directly. `get_policy`
  returns `Option` — `None` still means "nothing configured," and `Service`
  keeps applying `TenantPolicy::default_for_tenant` as the fallback.

- **New GTS type**, `prompt-scanner-sdk/src/gts.rs`: `PolicyProviderSpecV1`,
  same shape as `ScannerBackendSpecV1` (a `PluginV1<PolicyProviderSpecV1>`
  instance per provider, `vendor` + `priority`).

- **Selection is single-instance, not fan-out** — the opposite of scanner
  backends. Exactly one policy provider should be authoritative at a time
  (there's no sensible way to "aggregate" two different tenant policies).
  `Service::resolve_policy_provider` lists every registered
  `PolicyProviderSpecV1` instance, picks the lowest-`priority`-value one
  (ties broken by registry order), and resolves it via
  `ClientHub::try_get_scoped::<dyn PolicyProvider>()`, falling back to the
  built-in DB-backed provider if none is registered or the scoped client
  is missing. *(As implemented: this is hand-rolled inline in `service.rs`
  rather than calling `toolkit::plugins::choose_plugin_instance`, because
  that helper picks among instances of one already-known `vendor` — exactly
  right for scanner backends, where the caller knows which vendors it wants;
  wrong here, where there's no config-supplied vendor to filter by and the
  whole point is "pick whichever provider is registered, if any.")*

- **Built-in default provider**: wrap today's `TenantPolicyRepository` +
  `OrmTenantPolicyRepository` behind an internal `DbPolicyProvider` that
  implements the new trait (thin adapter — no schema change). Registered
  nowhere in `types-registry` (it's not a discoverable plugin instance,
  just the fallback `Service` reaches for when `resolve_policy_provider()`
  finds nothing). This keeps every existing deployment's behavior byte-for-byte
  identical with zero config changes.

- `Service`'s existing PEP checks
  (`policy_enforcer.access_scope_with(..., TENANT_POLICY, GET/UPDATE, ...)`)
  stay exactly where they are, in `get_policy`/`put_policy` — they authorize
  *prompt-scanner's own* `GET`/`PUT /policy` REST endpoints, independent of
  which provider ends up storing the data. A provider (e.g. an external policy-provider adapter)
  is free to also enforce its own authorization; that's out of scope here.

- **Why a plugin, not a `ClientHub` singleton dependency**: if
  `PolicyProvider` were just `deps = [policy_provider]` on `prompt-scanner`'s
  gear, then the demo console (or the future external policy-provider adapter) would become a
  hard, required dependency of the core scanning gear — exactly the
  plugin-isolation violation `docs/DESIGN.md` calls out for scanner backends.
  Going through `types-registry` + scoped `ClientHub` entries instead means
  `prompt-scanner` never needs to know the demo console gear exists.

### `TenantPolicy` addition: category action overrides

To let the demo's radio buttons actually change *what happens* on a PII hit
(not just drop/keep it), add one field:

```rust
pub struct TenantPolicy {
    ...
    /// Overrides a finding's own `suggested_action` when its category
    /// starts with a configured prefix. Applied before `aggregate_verdict`
    /// takes the max. Empty by default (today's behavior: detectors' own
    /// suggested actions are authoritative).
    #[serde(default)]
    pub category_action_overrides: HashMap<String, VerdictAction>,
}
```

`domain::policy::aggregate_verdict` gains one step before taking the max:
for each finding, if any override key is a prefix of `finding.category`,
replace `finding.suggested_action` with the override (longest-prefix-match
if more than one applies — same tie-break flavor as `thresholds` lookup, but
prefix instead of exact match). The demo console sets a single entry,
`"pii" -> chosen_action`, to drive its radio buttons; a future external policy-provider adapter
could set arbitrary per-category overrides the same way. This is additive
and backward compatible — existing policies with an empty map behave exactly
as they do today.

## 3. Demo console gear: shape

**Explicitly temporary** — this exists to make the GenAI Security demo
interactive, not as a real policy-provider integration. It should be easy to
delete outright once a real external adapter lands, without prompt-scanner needing any
change (that's the whole point of going through the plugin extension point
above rather than wiring it in directly).

- **Location**: `gears/prompt-scanner/plugins/demo-policy-console/` —
  alongside `regex-scanner`, `onnx-scanner`, `always-allow-plugin`. It's a
  plugin gear from `prompt-scanner`'s point of view (registers a
  `PolicyProviderSpecV1` instance + scoped `PolicyProvider` client), but
  also has `capabilities = [rest]` for its own UI/API — nothing about the
  plugin-isolation rule prevents a plugin gear from serving its own REST
  surface for itself; it only forbids a *regular* gear depending on a
  *plugin* gear directly.
- **Gear declaration**:
  `deps = [types_registry, prompt_scanner], capabilities = [rest]` — it
  needs `prompt_scanner` as a dep because the audit-log panel calls
  `PromptScannerClientV1::list_scans` directly via `ClientHub` (a
  regular-gear SDK client, same pattern `openai-interceptor-plugin` already
  uses to depend on `prompt_scanner`).
- **State**: one in-memory `RwLock<DemoPolicyState>` (this is a demo —
  no migration, no persistence beyond process lifetime, same "scaffold-level
  durability" tradeoff `Service`'s in-memory job queue already makes):

  ```rust
  struct DemoPolicyState {
      pii_enabled: bool,             // config-seeded, default: true
      pii_action: VerdictAction,     // config-seeded, default: Redact
      enabled_backends: Vec<String>, // config-seeded, no UI control
  }
  ```

  *(As implemented: `enabled_backends` ended up living in `DemoPolicyState`
  too, config-seeded from `DemoPolicyConsoleConfig::enabled_backends`
  (defaulting to the same three vendors `scripts/run-genai-security.sh` used
  to `curl -X PUT .../policy` by hand) — see the note below on why this
  became necessary.)*

- **`PolicyProvider` impl**: `get_policy` builds a `TenantPolicy` on the fly
  from `DemoPolicyState` — same policy for every tenant, since this is a
  single-toggle demo, not real per-tenant config:
  - `enabled_backends` comes straight from `DemoPolicyState.enabled_backends`
    (see above) — the toggle does **not** flip it on/off per call, since
    disabling a backend entirely also loses its non-PII findings (e.g.
    `regex-scanner`'s `secret.aws_key`). Instead:
    - `pii_enabled = false` → `category_action_overrides["pii"] = Allow`
      (PII findings still get produced and recorded for audit, but never
      change the verdict — "detection off" reads as "detection produces no
      effect," which is also more honest for the audit log than hiding rows).
    - `pii_enabled = true` → `category_action_overrides["pii"] = pii_action`
      (whichever radio button is selected).
  - `put_policy` is not meaningfully implemented — it echoes back the
    currently effective policy rather than erroring, so a caller unaware of
    the split still gets a sensible response. The demo mutates
    `DemoPolicyState` through its own `/state` endpoint below, not through
    `PUT /prompt-scanner/v1/policy`.
  - **Why `enabled_backends` had to move into `DemoPolicyState`** (deviation
    from the original plan, which assumed the DB-backed `enabled_backends`
    row would keep being the source of truth): once this gear becomes
    `prompt-scanner`'s *resolved* provider, `Service::load_policy` never
    consults `OrmTenantPolicyRepository` at all — so if `get_policy` had kept
    building `TenantPolicy::default_for_tenant` (empty `enabled_backends`),
    every scan would fan out to zero backends the moment this gear is
    enabled, silently breaking the whole demo pipeline. Seeding
    `enabled_backends` from this gear's own config closes that gap and
    means `scripts/run-genai-security.sh` no longer needs its manual
    `curl -X PUT .../policy` setup step when `demo-policy-console` is in
    `$FEATURES` (the script now skips it in that case).
- **REST surface** (`/demo-policy-console/v1/...`, its own `OperationBuilder`
  routes + OpenAPI, same as every other gear):
  - `GET /state` — current `{ pii_enabled, pii_action }`.
  - `PUT /state` — update either/both fields, validates `pii_action` is a
    real `VerdictAction`.
  - `GET /audit` — thin passthrough to
    `PromptScannerClientV1::list_scans` (paginated), reshaped for the UI
    (trace id, action, categories hit, timestamp).
- **Web UI**: one `GET /demo-policy-console/` handler returning
  `Html<&'static str>` with inline `<style>`/`<script>` — same pattern
  `gears/system/api-gateway/src/web.rs` uses for `serve_docs` (no
  static-file-serving infra exists in this codebase, and none is needed for
  one page). Contents:
  - A toggle switch bound to `pii_enabled` (`PUT /state` on change).
  - A radio group (`Allow` / `Audit` / `Warn` / `Redact` / `Block`) bound to
    `pii_action`, disabled when `pii_enabled` is off.
  - An audit log table polling `GET /audit` every 4s (trace id, action
    badge, categories, time) — read-only, no filtering UI needed for a demo.
  - Plain `fetch()` calls against its own REST endpoints above; no build
    step, no frontend framework, consistent with the "temporary" framing.
- **Naming gotcha hit during implementation**: the demo's own wire-format
  enum for `VerdictAction` was first named `VerdictActionDto`, colliding
  with `prompt-scanner`'s own `VerdictActionDto` at OpenAPI-schema-registration
  time (`utoipa` registers component schemas by name across the whole
  merged OpenAPI doc, not per-gear) — this panics the server at boot
  (`OpenAPI schema name collision`), not at compile time. Renamed to
  `DemoVerdictActionDto`. Worth remembering for any future gear that mirrors
  another gear's DTO shape under the same short name.

## 4. Sequencing (as implemented)

1. Added `category_action_overrides` to `TenantPolicy` + the prefix-override
   step in `aggregate_verdict`, with unit tests in `domain/policy.rs`.
2. Added `PolicyProvider` trait + `PolicyProviderSpecV1` GTS type to
   `prompt-scanner-sdk`; added `PromptScannerClientV1::list_scans` (needed
   by the audit panel, wasn't on the SDK trait before — only exposed over
   REST); wrapped the current DB path as `DbPolicyProvider` in a new
   `domain/policy_provider.rs`; dropped `Service`'s `PR` generic parameter
   entirely in favor of `default_policy_provider: Arc<dyn PolicyProvider>`;
   updated `load_policy`/`put_policy` to resolve via
   `resolve_policy_provider()`. Added `TenantPolicyDto::category_action_overrides`
   and a `tenant_policies.category_action_overrides` DB column (this gear
   had shipped no prior migrations, so the initial migration was edited in
   place rather than adding a second one).
3. Scaffolded `demo-policy-console` as a new plugin gear under
   `gears/prompt-scanner/plugins/`; added it to the workspace, to
   `apps/cf-gears-example-server` behind a new `demo-policy-console` Cargo
   feature (implies `prompt-scanner-example`), and to
   `scripts/run-genai-security.sh`'s default `$FEATURES`.
4. Verified end-to-end by booting `cf-gears-example-server` with
   `--features forward-proxy-example,demo-policy-console,static-tenants,static-authz`
   against `config/genai-security.yaml`: confirmed `GET /prompt-scanner/v1/policy`
   reflects the demo's config-seeded `enabled_backends` and
   `category_action_overrides`, and that toggling
   `PUT /demo-policy-console/v1/state` flips a real scan's verdict between
   `redact` and `allow` while the finding stays visible in
   `GET /demo-policy-console/v1/audit`.

## 5. Explicitly out of scope here

- The real external policy-provider adapter itself (this only shapes the extension
  point it will plug into).
- Per-tenant policy in the demo console (single global toggle only).
- Persisting demo state across restarts.
- Auth/RBAC on the demo console's own UI beyond whatever the platform
  applies by default — it's a demo control panel, not a customer-facing
  surface.
