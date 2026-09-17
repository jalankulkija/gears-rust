# prompt-scanner — Design

Scope: the shared GenAI scanning platform.

This document records the design and the reasoning behind it.

## Purpose

The core tenant-aware cloud inspection/verdict microservice behind the
consumer-facing gateway layer. Accepts prompts, responses, uploads, and MCP traffic
from multiple connector types (endpoint agents, first-party AI apps, customer
apps, AI/API gateways, remote MCP proxies, network proxies/firewalls). Runs a
common detector portfolio (DLP, secrets/credential leaks, prompt
injection/jailbreak, harmful content, malicious code/URLs) and returns
standardized, traceable verdicts (allow, block, redact, warn, audit).

## Integration with `llm-gateway`

`llm-gateway` already has a Hook Plugin extension point
(`LlmGatewayHookPluginClient`, `pre_request` / `post_response`) explicitly
built for content moderation and PII filtering. Rather than have
`prompt-scanner` depend on or reach into `llm-gateway`, the integration is a new
small plugin gear, `gears/llm-gateway/plugins/scan-hook-plugin/`, that
implements `LlmGatewayHookPluginClient` and internally calls
`prompt-scanner`'s own SDK (`PromptScannerClientV1`). This makes `llm-gateway`
(and therefore the first-party chat application that sits on it) connector #1,
without violating the platform's plugin-isolation rule (regular
gears cannot depend on or consume plugin gears directly).

## Directory structure

```
gears/prompt-scanner/
├── prompt-scanner-sdk/
│   └── src/
│       ├── api.rs          # PromptScannerClientV1 (public — connectors call this)
│       ├── plugin_api.rs   # DetectorPluginClient (implemented by each detector)
│       ├── models.rs       # ScanRequest, ScanVerdict, Finding, VerdictAction, ContentKind
│       ├── errors.rs       # PromptScannerError
│       └── gts.rs          # DetectorPluginSpecV1 GTS schema
├── prompt-scanner/
│   └── src/
│       ├── gear.rs
│       ├── config.rs
│       ├── api/rest/{dto.rs, handlers.rs, routes.rs, error.rs}
│       ├── domain/
│       │   ├── service.rs      # orchestrates fan-out + verdict aggregation
│       │   ├── policy.rs       # per-tenant detector config/thresholds
│       │   ├── error.rs
│       │   └── local_client.rs
│       └── infra/storage/
│           ├── entity/scan_record.rs
│           ├── mapper.rs
│           └── migrations/
└── plugins/
    ├── dlp-plugin/
    ├── secrets-plugin/
    ├── prompt-injection-plugin/
    ├── harmful-content-plugin/
    └── malicious-code-plugin/
```

Plus, in `gears/llm-gateway/plugins/`: the `scan-hook-plugin` described above.

## SDK trait (`prompt-scanner-sdk/src/api.rs`)

```rust
#[async_trait]
pub trait PromptScannerClientV1: Send + Sync {
    async fn scan(&self, ctx: &SecurityContext, req: ScanRequest) -> Result<ScanVerdict, PromptScannerError>;
    async fn scan_async(&self, ctx: &SecurityContext, req: ScanRequest) -> Result<ScanJobHandle, PromptScannerError>; // large uploads
    async fn get_verdict(&self, ctx: &SecurityContext, trace_id: Uuid) -> Result<ScanVerdict, PromptScannerError>;
    async fn get_policy(&self, ctx: &SecurityContext) -> Result<TenantPolicy, PromptScannerError>;
    async fn put_policy(&self, ctx: &SecurityContext, policy: TenantPolicy) -> Result<TenantPolicy, PromptScannerError>;
}

// plugin_api.rs — implemented by each detector plugin
#[async_trait]
pub trait DetectorPluginClient: Send + Sync {
    async fn detect(&self, ctx: &SecurityContext, input: DetectorInput) -> Result<DetectorFinding, PromptScannerError>;
}
```

## Domain model

- `ScanRequest { tenant_id, connector_id, content_kind: Prompt|Response|Upload|McpToolCall|McpToolResult, payload, metadata }`
- `ScanVerdict { trace_id, action: Allow|Block|Redact|Warn|Audit, findings: Vec<Finding>, redacted_payload: Option<...> }`
- `Finding { detector: String, category: String, severity, confidence, span: Option<Range> }`
- `TenantPolicy { enabled_detectors: Vec<String>, thresholds: HashMap<String, f32>, default_action }`
- `Service::scan()` — resolves `TenantPolicy`, fans out to **all enabled**
  detector plugins concurrently (with per-detector timeout), aggregates
  findings into one verdict per policy rules, persists a `ScanRecord`, emits
  an event via `event-broker` for the audit trail.

## Plugin architecture — one deviation worth calling out

`llm-gateway`'s plugins use **single-instance selection** (pick one
provider/hook by vendor+priority, via `choose_plugin_instance`).
`prompt-scanner` needs the opposite: **fan-out to every enabled detector
plugin** matching `DetectorPluginSpecV1`, not pick-one. So `Service::scan()`
won't use `choose_plugin_instance` — it queries `types-registry` for all
instances of the type, filters by the tenant's `enabled_detectors`, and calls
each via `get_scoped::<dyn DetectorPluginClient>()` concurrently with
`tokio::time::timeout` per call. A detector that times out or errors degrades
to an "unscored" finding rather than blocking the request, unless policy
marks it `required`.

## REST endpoints (`/prompt-scanner/v1/...`)

| Method | Path | Purpose |
|---|---|---|
| POST | `/scans` | Sync scan → verdict (main connector ingestion path) |
| POST | `/scans/async` | Queue large upload scan → job id |
| GET | `/scans/{trace_id}` | Fetch a historical verdict (traceability) |
| GET | `/scans` | OData-filterable audit search (admin UI) |
| GET | `/policy` | Get tenant's detector policy |
| PUT | `/policy` | Update tenant's detector policy |
| GET | `/detectors` | List registered detector plugin instances + health |

## DB entities

- `scan_record` (Scopable, `tenant_col = tenant_id`): trace_id, connector_id,
  content_kind, verdict_action, findings (jsonb), created_at — the audit
  trail the platform's requirements call for.

Tenant policies are not stored here: they come from the resolved
`PolicyProvider` (a plugin, or the built-in read-only `ConfigPolicyProvider`
serving the gear config's `fallback_policy`).

## Dependencies

`deps = ["types-registry"]` for plugin schema; consumes `event-broker`
(audit events) and `tenant-resolver`/platform `SecurityContext` for tenancy.
Does **not** depend on detector plugin crates or on `llm-gateway` — isolation
stays intact in both directions.

## Dual-deployment note

No special code needed for cloud-vs-customer-hosted — it's config-driven
(which detector plugin set is compiled in / enabled) plus the existing
Postgres-cluster-vs-single-instance DB config already in the platform
(`feat(cluster): implement Postgres cluster plugin`) applies here directly.

## Open questions / not yet decided

- Exact verdict-aggregation rules when multiple detectors disagree (e.g. one
  says `warn`, another says `block`) — needs a precedence policy.
- Shape of the async job model for large uploads (polling vs. webhook vs. SSE).
- Whether `scan_record` findings need per-tenant retention/redaction rules for
  compliance (customer-hosted regulated environments).
