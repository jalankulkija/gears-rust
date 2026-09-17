# prompt-scanner SDK

Transport-agnostic public API for the `prompt-scanner` gear. Consumers depend on this
crate, never on the gear itself.

- `PromptScannerClientV1` — `scan`, `scan_async`, `get_verdict`, policy read/write.
- `ScanRequest` / `ScanInput` / `Verdict` / `Finding` / `VerdictAction` — the wire and
  domain models, plus `redact_spans` for applying a `Redact` verdict to text.
- `ScannerBackendClient` + `ScannerBackendSpecV1` — the extension point a detector
  implements to join the fan-out.
- `PolicyProvider` + `PolicyProviderSpecV1` — the extension point for supplying tenant
  policy from somewhere other than the gear's own database. It deliberately takes no
  database runner, so a provider backed by an external policy service fits.

See [../docs/DESIGN.md](../docs/DESIGN.md) for the architecture and
[../docs/prompt-scanner-interface-reference.md](../docs/prompt-scanner-interface-reference.md)
for worked examples of both extension points.
