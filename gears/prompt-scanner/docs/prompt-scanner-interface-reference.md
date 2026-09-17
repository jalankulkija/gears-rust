# prompt-scanner — Interface Reference

Tenant-aware content-scanning gateway. Connectors submit content (prompts, LLM
responses, uploads, MCP tool calls); the gear fans out to **every** scanner
backend plugin enabled by the tenant's policy, aggregates their findings into a
single verdict, and persists an audit record.

Crates:
- `cf-gears-prompt-scanner-sdk` (`prompt_scanner_sdk`) — public API trait,
  plugin API traits, GTS specs, models, errors.
- `cf-gears-prompt-scanner` — the gear (REST + domain + storage).

---

## 1. REST API

Base path: `/prompt-scanner/v1`. All endpoints are authenticated
(`SecurityContext` extension); errors are RFC-9457 / AIP-193 `CanonicalError`
bodies. Tag: `Scan Gateway`.

| Method | Path                | Success | Purpose |
|--------|---------------------|---------|---------|
| POST   | `/scans`            | 201     | Sync scan → verdict |
| POST   | `/scans/async`      | 202     | Queue a scan → job handle |
| GET    | `/scans/{trace_id}` | 200     | Fetch a historical verdict |
| GET    | `/scans`            | 200     | OData-filterable audit search |
| GET    | `/backends`         | 200     | List registered backend instances |

### POST /scans

```http
POST /prompt-scanner/v1/scans
Content-Type: application/json

{
  "tenant_id": "018f2f1e-4d55-7c9a-8c3d-6b0a1c2d3e4f",
  "connector_id": "llm-gateway",
  "content_kind": "prompt",
  "payload": "Ping jane.doe@example.com about card 4111111111111111",
  "metadata": { "model": "gpt-4o", "source_ip": "10.0.0.7" }
}
```

`201 Created` (+ `Location: /prompt-scanner/v1/scans/{trace_id}`):

```json
{
  "trace_id": "019255aa-7f31-7a10-9c11-c4a1b2d3e4f5",
  "tenant_id": "018f2f1e-4d55-7c9a-8c3d-6b0a1c2d3e4f",
  "action": "block",
  "findings": [
    {
      "detector": "cf.builtin.regex_scanner",
      "category": "pii.email",
      "severity": "high",
      "confidence": 0.5,
      "span": { "start": 5, "end": 25 },
      "suggested_action": "redact"
    },
    {
      "detector": "cf.builtin.regex_scanner",
      "category": "financial.credit_card",
      "severity": "critical",
      "confidence": 0.95,
      "span": { "start": 37, "end": 53 },
      "suggested_action": "block"
    }
  ],
  "created_at": "2026-09-02T10:15:00Z"
}
```

When the aggregated `action` is `redact`, the response also carries
`redacted_payload`. Only the spans of `redact`-suggesting findings are replaced
with `[REDACTED]` — overlapping spans merge into one placeholder, and a
`redact` finding with no span falls back to redacting the whole text. A payload
whose only finding is the email above comes back as:

```json
{
  "action": "redact",
  "redacted_payload": "Ping [REDACTED] about card 4111111111111111"
}
```

**Field notes.** `content_kind` ∈ `prompt | response | upload | mcp_tool_call |
mcp_tool_result`. `payload` must be non-empty (unless `attachment` is set) and
`connector_id` must be non-blank, else `400`. Callers must be authorized to
create scans for `tenant_id` (PEP check on `owner_tenant_id`), else `403`.

**Uploads.** For `content_kind: "upload"`, send raw bytes and let the gear run
`file-parser`; `payload` is then ignored:

```json
{
  "tenant_id": "018f2f1e-4d55-7c9a-8c3d-6b0a1c2d3e4f",
  "connector_id": "endpoint-agent",
  "content_kind": "upload",
  "payload": "",
  "attachment": {
    "filename": "contract.pdf",
    "mime_type": "application/pdf",
    "data_base64": "JVBERi0xLjQK..."
  }
}
```

### POST /scans/async

Same request body. `202 Accepted`:

```json
{
  "job_id": "019255ab-0000-7a10-9c11-000000000001",
  "trace_id": "019255ab-0000-7a10-9c11-000000000002",
  "status": "queued"
}
```

Status values:
`queued | running | completed | failed`.

### GET /scans/{trace_id}

`200` returns the same `ScanVerdictDto` shape as `POST /scans`; `404` if no
verdict was recorded under that trace id.

### GET /scans (audit search)

```http
GET /prompt-scanner/v1/scans?$filter=tenant_id eq 018f2f1e-4d55-7c9a-8c3d-6b0a1c2d3e4f and created_at gt 2026-09-01T00:00:00Z&$orderby=created_at desc&$top=50
```

Filterable / orderable fields: `trace_id`, `tenant_id`, `connector_id`,
`created_at`. `$select` is supported. Response is a `Page<ScanVerdictDto>`:

```json
{
  "items": [ { "trace_id": "...", "action": "allow", "findings": [] } ],
  "page_info": { "next_cursor": "eyJ0IjoiMjAy...", "prev_cursor": null, "limit": 50 }
}
```

Paging is cursor-based: pass a returned `next_cursor` back as `$skiptoken`
(alias `cursor`); `$top` (alias `limit`) is capped by `max_page_size`.

### GET /backends

```json
[
  { "gts_id": "gts.cf.toolkit.plugins.plugin.v1~cf.core.prompt_scanner.scanner_backend.v1~cf.builtin.regex_scanner.detector.v1~",
    "vendor": "cf.builtin.regex_scanner", "priority": 500, "available": true },
  { "gts_id": "...always_allow.detector.v1~",
    "vendor": "cf.builtin.always_allow", "priority": 1000, "available": false }
]
```

`available: false` means the instance is registered in `types-registry` but its
scoped `ClientHub` entry isn't there, so the gateway cannot call it right now.

### Verdict aggregation rules

The rules below are driven by the calling tenant's `TenantPolicy`, which is not
part of this REST surface — it is owned by the resolved `PolicyProvider`
(see [§3.2](#32-policyprovider--where-tenant-policy-lives)).

0. A `content_kind` not in the policy's `content_kinds` is not scanned: no
   backend runs, the verdict is `allow` with no findings, and the scan is still
   recorded.
1. Drop findings whose `confidence` is below `thresholds[category]` (missing
   threshold ⇒ `0.0`, always passes).
2. Replace each surviving finding's `suggested_action` with the
   **longest-prefix-matching** entry in `category_action_overrides`
   (`"pii"` matches `pii.email`; `"pii.email"` beats `"pii"`).
3. Verdict = **max** action across survivors:
   `block > redact > warn > audit > allow`.
4. No survivors ⇒ `default_action`.

`severity` plays no part in aggregation — only `confidence` (step 1) and
`suggested_action` (steps 2-3) do. It is carried through for reporting.

Backend failure modes: a backend that errors or exceeds
`backend_timeout_ms` (default 5000) is silently "unscored" — unless it is in
`required_backends`, in which case the scan fails closed (`500`,
`RequiredBackendFailed`). A backend returning `Defer` is queued for background
processing under the same `trace_id`; its findings are merged into the existing
record and the verdict is recomputed.

### Errors

| Domain error | HTTP | Notes |
|---|---|---|
| `VerdictNotFound` / `JobNotFound` / `PolicyNotFound` | 404 | |
| `Validation` | 400 | field violation, code `VALIDATION` |
| `AttachmentParsingFailed` | 400 | code `ATTACHMENT_PARSE_FAILED` |
| `BackendTimeout` | 400 | code `TIMEOUT` |
| `Forbidden` | 403 | reason `ACCESS_DENIED` |
| `BackendUnavailable` / `RequiredBackendFailed` | 500 | |
| `PolicyUnavailable` | 503 | the policy provider's source is unreachable and it has no policy to serve; the reason is logged, not returned |
| `Database` / `Internal` | 500 | |

### Gear config

```yaml
prompt_scanner:
  default_page_size: 50
  max_page_size: 1000
  backend_timeout_ms: 5000      # per-backend call timeout
  job_poll_interval_ms: 200     # async job queue drain interval

# UNDER_CONSTRUCTION: This part is worked on currently. A lot of knobs are undexposed right now.
```

---

## 2. In-process API — `PromptScannerClientV1`

Consumed via `ClientHub` by connectors in the same process (e.g.
`llm-gateway`'s scan hook plugin, `demo-policy-console`'s audit panel).

```rust
#[async_trait]
pub trait PromptScannerClientV1: Send + Sync {
    async fn scan(&self, ctx: &SecurityContext, req: ScanRequest) -> Result<ScanVerdict, PromptScannerError>;
    async fn scan_async(&self, ctx: &SecurityContext, req: ScanRequest) -> Result<ScanJobHandle, PromptScannerError>;
    async fn get_verdict(&self, ctx: &SecurityContext, trace_id: Uuid) -> Result<ScanVerdict, PromptScannerError>;
    async fn list_scans(&self, ctx: &SecurityContext, query: &ODataQuery) -> Result<Page<ScanVerdict>, PromptScannerError>;
}
```

What you pass in, and what you get back:

```rust
/// Everything the caller sets. `Default` is not implemented — set every field.
pub struct ScanRequest {
    pub tenant_id: Uuid,
    /// Who is submitting, e.g. "llm-gateway". Must be non-blank.
    pub connector_id: String,
    pub content_kind: ContentKind,
    /// Must be non-empty unless `attachment` is set (then it is ignored).
    pub payload: String,
    /// Free-form, e.g. {"model": "gpt-4o"}. Pass `HashMap::new()` if unused.
    pub metadata: HashMap<String, String>,
    /// Only meaningful for `ContentKind::Upload`; `None` otherwise.
    pub attachment: Option<Attachment>,
}

pub enum ContentKind { Prompt, Response, Upload, McpToolCall, McpToolResult }

pub struct Attachment {
    pub filename: Option<String>,   // helps file-parser pick a backend
    pub mime_type: Option<String>,
    pub data_base64: String,        // standard alphabet, padded
}

/// Returned by `scan` / `get_verdict` — all fields are set by the gear.
pub struct ScanVerdict {
    pub trace_id: Uuid,
    pub tenant_id: Uuid,
    pub action: VerdictAction,
    pub findings: Vec<Finding>,
    /// `Some` only when `action == VerdictAction::Redact`.
    pub redacted_payload: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// `Ord` by declaration order — aggregation takes the max.
pub enum VerdictAction { Allow, Audit, Warn, Redact, Block }

/// Returned by `scan_async`; poll `get_verdict(trace_id)`.
pub struct ScanJobHandle {
    pub job_id: Uuid,
    pub trace_id: Uuid,
    pub status: ScanJobStatus,
}

pub enum ScanJobStatus { Queued, Running, Completed, Failed }
```

```rust
let client = ctx.client_hub().get::<dyn PromptScannerClientV1>()?;
let mut user_prompt: String = /* the text about to be sent to the model */;

let verdict = client
    .scan(&sec_ctx, ScanRequest {
        tenant_id,
        connector_id: "llm-gateway".into(),
        content_kind: ContentKind::Prompt,
        payload: user_prompt.clone(),
        metadata: HashMap::new(),
        attachment: None,
    })
    .await?;

match verdict.action {
    VerdictAction::Block => return Err(blocked()),
    VerdictAction::Redact => user_prompt = verdict.redacted_payload.unwrap_or_default(),
    _ => {}
}
```

---

## 3. Extension points

| # | Extension point | Trait | Selection | Owner of the seam |
|---|---|---|---|---|
| 1 | Scanner backend (detector) | `prompt_scanner_sdk::ScannerBackendClient` | **fan-out** to all instances whose `vendor` is in `enabled_backends` | `ScannerBackendSpecV1` GTS + scoped `ClientHub` |
| 2 | Tenant policy source | `prompt_scanner_sdk::PolicyProvider` | **single** instance, lowest `priority` value wins; DB-backed fallback | `PolicyProviderSpecV1` GTS + scoped `ClientHub` |

Registration pattern is identical: build a `PluginV1<Spec>`
registration, push it to `types-registry`, then register the implementation
into `ClientHub` under `ClientScope::gts_id(&instance_id)`.

---

### 3.1 `ScannerBackendClient` — a scanner backend

```rust
#[async_trait]
pub trait ScannerBackendClient: Send + Sync {
    async fn detect(&self, ctx: &SecurityContext, input: ScanInput)
        -> Result<BackendOutcome, PromptScannerError>;
}
```

What the gateway hands you, and what you must build:

```rust
/// Handed to `detect`. A narrowed `ScanRequest`: no connector id, and
/// `payload` is always plain text (attachments are already extracted).
pub struct ScanInput {
    pub tenant_id: Uuid,
    pub content_kind: ContentKind,
    pub payload: String,
    pub metadata: HashMap<String, String>,
}

pub enum BackendOutcome {
    /// EVERY match found in this payload. Empty vec = clean.
    Findings(Vec<Finding>),
    /// Too expensive to score inline; the gateway re-queues this call in the
    /// background under the same trace id.
    Defer,
}

/// What you construct per match.
pub struct Finding {
    /// Leave empty (`String::new()`) — the gateway fills in your vendor.
    pub detector: String,
    /// Dotted, e.g. "pii.email", "secret.aws_key". Policy overrides and
    /// thresholds match on this by prefix / exact key.
    pub category: String,
    /// Reporting only — aggregation ignores it.
    pub severity: Severity,
    /// In [0.0, 1.0]; compared against the tenant's `thresholds[category]`.
    pub confidence: f32,
    /// Byte offsets into `input.payload`. Set it whenever you can — `None`
    /// forces whole-payload blanking instead of surgical redaction.
    pub span: Option<Span>,
    /// Your vote; the verdict is the max across all findings.
    pub suggested_action: VerdictAction,
}

pub struct Span { pub start: u32, pub end: u32 }

pub enum Severity { Low, Medium, High, Critical }
```

Errors and timeouts are treated as "no finding" unless the tenant marked you
`required` — so returning `Err` is a valid way to say "I could not score this".

**Example — a word-list backend.** Flags each occurrence of a configured word.

```rust
// src/domain/service.rs
use async_trait::async_trait;
use prompt_scanner_sdk::{
    BackendOutcome, Finding, PromptScannerError, ScanInput, ScannerBackendClient,
    Severity, Span, VerdictAction,
};
use toolkit_macros::domain_model;
use toolkit_security::SecurityContext;

#[domain_model]
pub struct Service {
    /// Lowercased banned words.
    words: Vec<String>,
}

impl Service {
    #[must_use]
    pub fn new(words: Vec<String>) -> Self {
        Self { words: words.iter().map(|w| w.to_lowercase()).collect() }
    }

    fn findings_for(&self, payload: &str) -> Vec<Finding> {
        let haystack = payload.to_lowercase();
        let mut out = Vec::new();
        for word in &self.words {
            let mut from = 0;
            while let Some(rel) = haystack[from..].find(word.as_str()) {
                let start = from + rel;
                let end = start + word.len();
                out.push(Finding {
                    detector: String::new(), // filled in by prompt-scanner
                    category: format!("wordlist.{word}"),
                    severity: Severity::Medium,
                    confidence: 1.0,
                    span: Some(Span { start: start as u32, end: end as u32 }),
                    suggested_action: VerdictAction::Redact,
                });
                from = end;
            }
        }
        out
    }
}

#[async_trait]
impl ScannerBackendClient for Service {
    async fn detect(
        &self,
        _ctx: &SecurityContext,
        input: ScanInput,
    ) -> Result<BackendOutcome, PromptScannerError> {
        Ok(BackendOutcome::Findings(self.findings_for(&input.payload)))
    }
}
```

Registration (`src/gear.rs`):

```rust
#[toolkit::gear(name = "wordlist-scanner", deps = [types_registry])]
pub struct WordlistScanner {
    service: OnceLock<Arc<Service>>,
}

#[async_trait]
impl Gear for WordlistScanner {
    async fn init(&self, ctx: &GearCtx) -> anyhow::Result<()> {
        let cfg: WordlistScannerConfig = ctx.config_or_default()?; // vendor, priority, words
        let service = Arc::new(Service::new(cfg.words.clone()));

        let (instance_id, instance_json) = PluginV1::<ScannerBackendSpecV1>::build_registration(
            "cf.demo.wordlist.detector.v1",
            cfg.vendor.clone(),   // matched against TenantPolicy.enabled_backends
            cfg.priority,         // informational for backends (fan-out, not pick-one)
        )?;

        let registry = ctx.client_hub().get::<dyn TypesRegistryClient>()?;
        RegisterResult::ensure_all_ok(&registry.register(vec![instance_json]).await?)?;

        self.service.set(Arc::clone(&service)).ok();
        let api: Arc<dyn ScannerBackendClient> = service;
        ctx.client_hub()
            .register_scoped::<dyn ScannerBackendClient>(ClientScope::gts_id(&instance_id), api);
        Ok(())
    }
}
```

```yaml
gears:
  wordlist-scanner:
    config:
      vendor: cf.demo.wordlist
      priority: 700
      words: ["apples", "bananas"]
```

Then enable it for a tenant by adding `"cf.demo.wordlist"` to that tenant's
`TenantPolicy.enabled_backends` in whichever `PolicyProvider` is authoritative.

For the `Defer` path, return it instead of scanning when the payload is too big:

```rust
if input.payload.len() > self.max_sync_bytes {
    return Ok(BackendOutcome::Defer);
}
```

**Instance id shape.**
`gts.cf.toolkit.plugins.plugin.v1~cf.core.prompt_scanner.scanner_backend.v1~<vendor>.<package>.detector.v1~`

---

### 3.2 `PolicyProvider` — where tenant policy lives

```rust
#[async_trait]
pub trait PolicyProvider: Send + Sync {
    async fn get_policy(&self, ctx: &SecurityContext, tenant_id: Uuid)
        -> Result<Option<TenantPolicy>, PromptScannerError>;
    async fn put_policy(&self, ctx: &SecurityContext, policy: TenantPolicy)
        -> Result<TenantPolicy, PromptScannerError>;
}
```

The one struct you produce and consume:

```rust
pub struct TenantPolicy {
    pub tenant_id: Uuid,
    /// Vendors of the scanner backends to fan out to. Empty = nothing is
    /// scanned. Matched against each registered `ScannerBackendSpecV1`'s
    /// `vendor`.
    pub enabled_backends: Vec<String>,
    /// Subset of `enabled_backends` that must respond; if one errors or
    /// times out the scan fails closed instead of degrading to "unscored".
    pub required_backends: Vec<String>,
    /// Exact `Finding::category` -> minimum confidence, in [0.0, 1.0].
    /// Missing category ⇒ 0.0 (always passes).
    pub thresholds: HashMap<String, f32>,
    /// Action when no finding survives threshold filtering.
    pub default_action: VerdictAction,
    /// Category *prefix* -> action, overriding the detector's own
    /// `suggested_action` (longest prefix wins). Empty = detectors decide.
    pub category_action_overrides: HashMap<String, VerdictAction>,
    /// Content kinds the policy governs; a scan of any other kind runs no
    /// backends and is allowed. Absent (serde) ⇒ every kind.
    pub content_kinds: Vec<ContentKind>,
}

// Start from a permissive baseline rather than filling all seven fields:
let mut policy = TenantPolicy::default_for_tenant(tenant_id);
// -> no backends, no thresholds, no overrides, default_action = Allow,
//    every content kind
```

Exactly one provider is authoritative: of the registered `PolicyProviderSpecV1`
instances, the one with the **lowest** `priority` value wins; with none
registered (or its client missing), the gear's built-in read-only
`ConfigPolicyProvider` is used: it serves the gear config's `fallback_policy`
to every tenant (or `None` when that is unset) and refuses writes, so
`PUT /policy` answers 400. prompt-scanner stores no tenant policies.
`Ok(None)` means "nothing configured" — the caller falls back to
`TenantPolicy::default_for_tenant`. The trait takes no DB runner on purpose, so
a provider backed by an external policy service fits; such a provider owns
its own authorization.

**Example — a static, config-driven provider.** Same policy for every tenant,
read-only.

```rust
use async_trait::async_trait;
use prompt_scanner_sdk::{PolicyProvider, PromptScannerError, TenantPolicy, VerdictAction};
use toolkit_macros::domain_model;
use toolkit_security::SecurityContext;
use uuid::Uuid;

#[domain_model]
pub struct Service {
    enabled_backends: Vec<String>,
}

#[async_trait]
impl PolicyProvider for Service {
    async fn get_policy(
        &self,
        _ctx: &SecurityContext,
        tenant_id: Uuid,
    ) -> Result<Option<TenantPolicy>, PromptScannerError> {
        let mut policy = TenantPolicy::default_for_tenant(tenant_id);
        policy.enabled_backends.clone_from(&self.enabled_backends);
        policy
            .category_action_overrides
            .insert("pii".to_owned(), VerdictAction::Redact);
        Ok(Some(policy))
    }

    async fn put_policy(
        &self,
        _ctx: &SecurityContext,
        _policy: TenantPolicy,
    ) -> Result<TenantPolicy, PromptScannerError> {
        Err(PromptScannerError::validation("policy is read-only in this provider"))
    }
}
```

Registration is the same as §3.1 with `PolicyProviderSpecV1` and
`register_scoped::<dyn PolicyProvider>` (see `demo-policy-console/src/gear.rs`).

**Instance id shape.**
`gts.cf.toolkit.plugins.plugin.v1~cf.core.prompt_scanner.policy_provider.v1~<vendor>.<package>.provider.v1~`

---

## 4. Configuring the built-in scanner backends

Both plugins are ordinary gears: their settings live under
`gears.<gear-name>.config` in the server YAML (see
`config/genai-security.yaml` for a working example). Every field has a default,
so `config: {}` boots a usable backend.

Two fields are common to both and are what ties a backend to a tenant:

| Field | Default (onnx / regex) | Meaning |
|---|---|---|
| `vendor` | `cf.builtin.onnx_pii` / `cf.builtin.regex_scanner` | The name a tenant puts in `TenantPolicy.enabled_backends`, and the value the gateway stamps into `Finding::detector`. |
| `priority` | `1000` / `500` | Informational for scanner backends — resolution is fan-out, not pick-one. |

### 4.1 `onnx-scanner`

Model-based PII detection. The plugin **never loads a model itself** — it calls
`onnx-runner`, so every `model_id` here must already be listed in
`onnx-runner`'s own `models:` config and loaded at boot.

```yaml
gears:
  onnx-runner:
    config:
      models:
        - model_id: "gliner2-pii"
          model_dir: "./gears/onnx-runner/models/gliner2-pii"
          model_file: "model.quantized.onnx"
          sha256: "489edc522a34b8c6a0630c4710998abf0227b11acf45600ba2bce95bcd943930"
          expected_inputs: ["input_ids", "attention_mask", "text_word_indices", "schema_special_indices"]
          expected_outputs: ["span_probs", "count_logits"]
          output_dtypes: { span_probs: f32, count_logits: f32 }
          replicas: 1

  onnx-scanner:
    config:
      vendor: "cf.builtin.onnx_pii"
      priority: 1000
      # Prepended to every entity label: "email" -> "pii.email".
      category_prefix: "pii"
      # Payloads longer than this (in words) return BackendOutcome::Defer
      # instead of calling the model.
      defer_threshold_words: 2000
      # Model score -> suggested_action mapping (checked in this order):
      #   >= redact_threshold -> Redact
      #   >= warn_threshold   -> Warn
      #   below both          -> dropped entirely, not reported as noise
      redact_threshold: 0.6
      warn_threshold: 0.3
      backend:
        kind: "gliner2"
        model_id: "gliner2-pii"
        labels: ["person", "email", "phone_number", "address", "iban", "api_key"]
        # Spans scoring below this are never emitted at all — applied inside
        # the model decoder, before redact/warn_threshold are considered.
        decode_threshold: 0.5
```

`backend.kind` selects the model family (the `PiiBackend` seam), and each kind
carries its own fields:

| `kind` | Fields | Notes |
|---|---|---|
| `gliner2` | `model_id` (default `gliner2-pii`), `labels`, `decode_threshold` (default `0.5`) | Zero-shot span model. `labels` conditions the schema prompt **and** becomes `Finding::category` verbatim (lowercased, prefixed). Change the labels and you change what is detected — no retraining. Model card operating points: `0.3` / `0.5` / `0.7`. |
| `bio_token_classifier` | `model_id` (default `minilm-l6-pii`), `labels` | Fixed-label BIO token classifier. `labels` is **not** a choice of what to detect — it must be the model's own label set, in the exact order its output logits are indexed (`["O", "B-EMAIL", "I-EMAIL", ...]`). Point `onnx-runner` at a different BIO model and you must replace this list to match, or decoding is silently wrong. |

The BIO variant, for comparison runs:

```yaml
gears:
  onnx-scanner:
    config:
      backend:
        kind: "bio_token_classifier"
        model_id: "minilm-l6-pii"
        labels: ["O", "B-EMAIL", "I-EMAIL", "B-PHONE", "I-PHONE",
                 "B-PERSON", "I-PERSON", "B-ADDRESS", "I-ADDRESS",
                 "B-DATE", "I-DATE"]
```

Only one `backend` is active per plugin instance. To run both families at once,
boot two instances with **different `vendor` values** and enable both in the
tenant policy — the gateway fans out to each.

Threshold interaction to keep in mind: `decode_threshold` is a hard floor
inside the model, `redact_threshold` / `warn_threshold` only ever see scores
that cleared it, and the tenant's own `thresholds[category]` filters again
during aggregation. Set `decode_threshold` above a tenant threshold and that
tenant's threshold becomes dead config.

### 4.2 `regex-scanner`

Deterministic, near-zero-cost detection: every pattern runs against the
payload, and every match becomes its own `Finding`.

```yaml
gears:
  regex-scanner:
    config:
      vendor: "cf.builtin.regex_scanner"
      priority: 500
      # Minimum Shannon entropy (bits/char) for patterns using the
      # `entropy` validator.
      entropy_threshold: 3.0
      patterns:
        - category: "pii.email"
          regex: '[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}'
          tier: "pattern"
          action: "redact"

        - category: "pii.ip_address"
          regex: '\b(?:\d{1,3}\.){3}\d{1,3}\b'
          # A regex match alone is not enough — confirm it parses as an IP.
          validator: "ipv4"
          tier: "validated"
          action: "redact"

        - category: "financial.credit_card"
          regex: '\b(?:\d[ -]?){12,18}\d\b'
          validator: "luhn"
          tier: "validated"
          action: "block"

        - category: "financial.iban"
          regex: '\b[A-Z]{2}\d{2}[A-Z0-9]{11,30}\b'
          validator: "iban"
          tier: "validated"
          action: "redact"

        - category: "secret.aws_key"
          regex: '\bAKIA[0-9A-Z]{16}\b'
          tier: "structural"
          action: "block"

        - category: "secret.jwt"
          regex: '\b[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b'
          validator: "jwt"
          tier: "structural"
          action: "block"

        - category: "secret.private_key"
          regex: '(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----'
          tier: "structural"
          action: "block"

        # Entropy-gated key=value fallback. `capture: 1` means the validator
        # runs on capture group 1 (the value) while the reported span covers
        # the whole match.
        - category: "secret.generic"
          regex: '(?i)\b(?:api[_-]?key|secret|token|password)\s*[:=]\s*["'']?([A-Za-z0-9/+_-]{16,})'
          capture: 1
          validator: "entropy"
          tier: "pattern"
          action: "warn"
```

Per-pattern fields:

| Field | Required | Meaning |
|---|---|---|
| `category` | yes | Becomes `Finding::category` verbatim. Keep it dotted (`family.thing`) so tenant `category_action_overrides` can target a whole family by prefix. |
| `regex` | yes | Rust `regex` crate syntax — no backreferences, no lookaround. Invalid patterns fail gear init rather than being skipped. Quote with YAML single quotes so backslashes survive. |
| `tier` | no (default `pattern`) | Confidence reported on the finding: `pattern` → `0.5`, `structural` → `0.75`, `validated` → `0.95`. Compared against the tenant's `thresholds[category]`. |
| `action` | yes | The finding's `suggested_action` (`allow`/`audit`/`warn`/`redact`/`block`), overridable per tenant. |
| `validator` | no | Post-match check; a match that fails it is dropped. One of `luhn`, `iban`, `ipv4`, `ipv6`, `jwt`, `entropy`. Use `validated` as the `tier` whenever a checksum validator is attached. |
| `capture` | no (default `0`) | Which capture group the validator inspects. The reported span always covers group `0`. |

Notes:
- Order does not matter — every pattern is evaluated, and overlapping matches
  each produce their own finding (the gateway merges overlapping spans only at
  redaction time).
- Regexes are compiled once at gear init, so a bad pattern is a startup
  failure, not a per-request one.
- **Omitting `patterns` is not the same as an empty list.** With the key
  absent the backend loads its built-in set (the seventeen patterns above plus
  the vendor-credential shapes for GitHub, OpenAI, Anthropic, Slack and Stripe,
  and a connection-string pattern), so a deployment that does not configure
  patterns still detects. An *explicitly* empty list (`patterns: []`) is valid
  and disables the backend's detection without removing it from the tenant
  policy — and is logged at `warn` on startup, because a silently inert
  detector is indistinguishable from a working one in the audit trail.
- Every pattern is evaluated, but not in a separate pass: the set is compiled
  into one `RegexSet` that reports which patterns occur in a single scan, and
  only those are re-run to extract spans and apply validators. On a payload
  with nothing to find — the common case — that is one pass rather than one per
  pattern. `examples/bench_regexset.rs` measures it.
- To run two different pattern sets side by side, boot two instances with
  different `vendor` values.

---
