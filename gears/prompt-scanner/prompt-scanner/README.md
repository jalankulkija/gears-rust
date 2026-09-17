# prompt-scanner

Tenant-aware inspection and verdict gear. Content goes in — prompts, model responses,
file uploads — and a single verdict comes out: `Allow`, `Audit`, `Warn`, `Redact` or
`Block`, with the findings that produced it recorded for audit.

Detection itself lives in plugins. `prompt-scanner` owns the parts that must not be
duplicated per detector: tenant policy, the concurrent fan-out across enabled backends,
per-backend timeouts, verdict aggregation, redaction, attachment text extraction, and the
`scan_record` audit trail.

See [docs/DESIGN.md](../docs/DESIGN.md) for the architecture and
[docs/prompt-scanner-interface-reference.md](../docs/prompt-scanner-interface-reference.md)
for the API surface.

## What it does

- **Inspection API** — `POST /scan` synchronously, or `scan_async` + `get_verdict` when a
  backend defers slow work. OData-filterable `GET /scans` over the audit trail.
- **Scanner backends as plugins** — any gear implementing `ScannerBackendClient` and
  registering a `ScannerBackendSpecV1` instance joins the fan-out. Shipped backends:
  `regex-scanner` (deterministic) and `onnx-scanner` (ML).
- **Per-tenant policy** — which backends run, per-category confidence thresholds,
  category→action overrides, and a `default_action` fallback. Supplied by the built-in
  DB-backed provider, or by any gear registering a `PolicyProviderSpecV1` instance.
- **Verdict aggregation** — findings below a category's threshold are dropped; the
  aggregate action is the maximum of what remains. One backend voting `Block` wins.
- **Degrades loudly** — a backend that errors or times out is dropped with a warning, and
  a *required* backend that cannot be resolved fails the scan rather than passing content
  as clean.

## Configuration

```yaml
prompt-scanner:
  database:
    server: sqlite_users
    file: "prompt_scanner.db"
  config: {}
```

A backend must be enabled in the tenant's policy before it runs; an empty
`enabled_backends` means no detection, by design rather than by accident.
