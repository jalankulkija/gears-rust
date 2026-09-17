Created:  2026-08-24 by Constructor Tech
Updated:  2026-08-24 by Constructor Tech

# GenAI Security Scanner — Program Design

<!-- toc -->

- [1. Scope and Sources](#1-scope-and-sources)
  - [1.1 What This Document Is](#11-what-this-document-is)
  - [1.2 Upstream Requirements](#12-upstream-requirements)
  - [1.3 What We Implement, and What We Do Not](#13-what-we-implement-and-what-we-do-not)
- [2. Architecture](#2-architecture)
  - [2.1 Component Map](#21-component-map)
  - [2.2 The Consumer/Platform Split](#22-the-consumerplatform-split)
  - [2.3 Request Path](#23-request-path)
- [3. Design Elements](#3-design-elements)
  - [3.1 Principles](#31-principles)
  - [3.2 Components](#32-components)
  - [3.3 Extension Contracts](#33-extension-contracts)
  - [3.4 Sequences](#34-sequences)
- [4. Requirement Coverage](#4-requirement-coverage)
- [5. Constraints](#5-constraints)

<!-- /toc -->

## 1. Scope and Sources

### 1.1 What This Document Is

The program-level architecture for the GenAI Security Scanner: the two product gears
(`prompt-scanner`, `onnx-runner`) that implement the GenAI protection scanning backend,
plus the demo/test harness (`forward-proxy`) used to exercise them.

This is the design layer of the repo's standard chain:

```
upstream requirements  →  this DESIGN  →  DECOMPOSITION  →  features/*.md  →  code  →  ADR
```

Per-gear design detail lives in each gear's own `docs/DESIGN.md`. This document owns only
what spans gears: the component split, the extension contracts between them, and the
mapping back to the upstream requirements.

### 1.2 Upstream Requirements

Requirements are **referenced, not mirrored**. Product requirements are maintained
outside this repository; this design cites their stable identifiers only.

- Upstream requirement IDs are of the form `cpt-genaigw-fr-*` / `cpt-genaigw-nfr-*`.
- Design elements defined here use `cpt-cf-genai-security-*`, and per gear
  `cpt-cf-<gear>-*`.

Because the requirement documents are not part of this repository, [§4](#4-requirement-coverage)
is the authoritative statement of what this program implements.

### 1.3 What We Implement, and What We Do Not

**In scope — the product:**

- `prompt-scanner`: the scanning backend. Inspection API, verdicts, scanner extension
  contract, tenant scoping, persistence, and audit trail.
- `onnx-runner`: ONNX model execution as a reusable platform capability.

**In scope — demo and test only, not a deliverable:**

- `forward-proxy` and `openai-interceptor-plugin`. A MITM proxy that captures ChatGPT
  traffic and calls the scanner. It exists to demonstrate and exercise the platform end to
  end; it is **not** part of the product and is not intended to ship. It is nevertheless
  architecturally load-bearing as *evidence*: because it consumes the scanning API the
  same way any external consumer would, it demonstrates that the API is usable
  independently of any proxy — the central claim of `cpt-genaigw-fr-inspection-api`.
  See [§2.2](#22-the-consumerplatform-split).

**Out of scope, owned upstream:**

- Application onboarding, SDK distribution, gateway adapters, provider compatibility
  profiles, commercial and billing experience. These belong to the consumer layer.
- Customer proxy, SWG, and firewall connectors.
- Endpoint capture and endpoint/cloud verdict combination.
- Any production traffic-interception product. Acting as a generic forward, reverse,
  workforce, MCP, SWG, or firewall proxy is explicitly excluded, as is any design that
  requires applications to route through a vendor-operated proxy. `forward-proxy` must
  not grow toward one.
- Fully air-gapped operation.

## 2. Architecture

### 2.1 Component Map

```
   ┌ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ┐
     DEMO / TEST ONLY — not a product deliverable
   │                                                     │
     browser ──▶ forward-proxy
   │              └─ openai-interceptor-plugin ──┐       │
   └ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ │ ─ ─ ─ ┘
                                                 │
     Product consumers (out of program scope):   │
       SDK callers, AI/API gateways,             │
       network connectors, first-party features  │
                                                 │
                              PromptScannerClientV1 (SDK)
                              or POST /prompt-scanner/v1/scans
                                                 │
                    ┌────────────────────────────▼────────────┐
                    │  prompt-scanner  (the platform)         │
                    │                                         │
                    │   PolicyProvider ◀── config fallback     │
                    │        │         (future: external      │
                    │        ▼          policy provider)      │
                    │   Service::scan  ──fan-out──┐           │
                    │        │                    │           │
                    │   scan_record (audit)       │           │
                    └─────────────────────────────┼───────────┘
                                                  │
                            ScannerBackendClient  │  (fan-out, concurrent)
                    ┌─────────────────────────────┼───────────┐
                    │                             │           │
              regex-scanner              onnx-scanner    always-allow
              (deterministic,            (ML, PII)       (test double)
               Tier 0)                        │
                                              │ OnnxRunnerClientV1
                                              ▼
                                        onnx-runner
                                        (generic tensor execution
                                         + tokenization)
```

### 2.2 The Consumer/Platform Split

The most load-bearing constraint on this program:

> The scanning backend does not have to proxy or forward LLM traffic. Consumers capture
> traffic, call the scanner, and apply the returned verdict.

`prompt-scanner` therefore **never** terminates TLS, forwards traffic, holds provider
credentials, or owns a network session. Consumers do all of that. Real consumers —
SDK callers, gateways, connectors, first-party features — are owned by the upstream
consumer layer and are outside this program.

`forward-proxy` is a stand-in for one. It is structurally interchangeable with any other
consumer, which is exactly why it works as evidence: `forward-proxy` does not depend on
`prompt-scanner` at all. The dependency runs `openai-interceptor-plugin →
prompt-scanner-sdk`, keeping the proxy core generic and demonstrating the API is
proxy-independent.

If that demonstration were the only reason the proxy existed it would still be worth
keeping. It also happens to be the fastest way to exercise the platform against real
traffic during development.

### 2.3 Request Path

1. A consumer intercepts or receives content and builds a `ScanRequest`
   (`tenant_id`, `connector_id`, `content_kind`, payload, attachments, metadata).
2. `Service::scan` resolves the effective `TenantPolicy` through `PolicyProvider`.
3. Attachments are extracted to text via `file-parser`.
4. Enabled backends fan out **concurrently**, each under its own timeout. A backend may
   return findings, an error, or `Defer` (too large for the synchronous path).
5. Findings aggregate into one verdict by precedence:
   `Block > Redact > Warn > Audit > Allow`.
6. `Redact` verdicts produce transformed content via span-based redaction.
7. A `scan_record` is persisted for the audit trail.
8. Deferred backends are drained by the background job worker, which re-aggregates and
   updates the same `scan_record` in place under the original `trace_id`.
9. The consumer applies the verdict and reports the action actually applied.

## 3. Design Elements

### 3.1 Principles

- [ ] `p1` - **ID**: `cpt-cf-genai-security-principle-consumer-enforces`

  The platform decides; the consumer enforces. No enforcement, capture, forwarding, or
  credential handling in `prompt-scanner`.

- [ ] `p1` - **ID**: `cpt-cf-genai-security-principle-deterministic-first`

  Deterministic detection runs first and always. Its recall is language-invariant where
  every evaluated ML model degrades outside its training language. A cheap regex tier is
  never held hostage by a slow ML tier.

- [ ] `p1` - **ID**: `cpt-cf-genai-security-principle-no-silent-clean`

  A scan that skipped, timed out, or failed a backend must never be indistinguishable
  from a clean scan.

- [ ] `p1` - **ID**: `cpt-cf-genai-security-principle-no-raw-content`

  Raw prompt/response content never enters logs, alerts, health endpoints, metrics, or
  diagnostics.

- [ ] `p1` - **ID**: `cpt-cf-genai-security-principle-generic-execution-layer`

  `onnx-runner` knows tensors and tokenizers, never model families. Prompt construction
  and output decoding live in the consumer. A new model family touches one plugin, not
  the platform.

### 3.2 Components

- [ ] `p1` - **ID**: `cpt-cf-genai-security-component-prompt-scanner`

  **Product.** The inspection platform. Owns the API, policy resolution, backend fan-out,
  verdict aggregation, redaction, the async job queue, and audit persistence.

- [ ] `p1` - **ID**: `cpt-cf-genai-security-component-onnx-runner`

  **Product.** Generic ONNX execution: model lifecycle, session replica pool,
  execution-provider selection, checksum and graph-contract verification, tokenization.

- [ ] `p1` - **ID**: `cpt-cf-genai-security-component-scanner-backends`

  **Product.** The detector portfolio behind `ScannerBackendClient`: `regex-scanner`
  (deterministic), `onnx-scanner` (ML), `always-allow-plugin` (test double).

- [ ] `p1` - **ID**: `cpt-cf-genai-security-component-forward-proxy`

  **Demo/test only.** Generic MITM forward proxy: CA lifecycle, host matching, ordered
  handler-plugin pipeline. Provider-agnostic; knows nothing about prompts or scanning.
  Not a deliverable; excluded from product acceptance criteria, packaging, and
  production-readiness work.

### 3.3 Extension Contracts

- [ ] `p1` - **ID**: `cpt-cf-genai-security-contract-scanner-backend`

  `ScannerBackendClient` — the scanner extension contract
  (`cpt-genaigw-fr-scanner-extension`). Fan-out
  semantics, not pick-one: every enabled backend runs concurrently per tenant policy.
  Returns `BackendOutcome::{Finding, Defer}` or an error; the three are handled distinctly.

- [ ] `p1` - **ID**: `cpt-cf-genai-security-contract-policy-provider`

  `PolicyProvider` — pluggable tenant-policy sourcing. `ConfigPolicyProvider` (one read-only
  policy from config; prompt-scanner stores none) is the default;
  an external policy-provider adapter substitutes without `Service` changes. This is the
  seam through which the upstream policy-source requirement — policy originates upstream,
  not from an independent customer-side source — is satisfied.

- [ ] `p1` - **ID**: `cpt-cf-genai-security-contract-onnx-runner`

  `OnnxRunnerClientV1` — `run(model_id, tensors)`, `encode(model_id, text)`,
  `list_models()`. Models are operator-configured at startup, never loaded from a
  caller-supplied path.

- [ ] `p1` - **ID**: `cpt-cf-genai-security-contract-proxy-handler`

  `ProxyHandlerPluginClient` — ordered and sequential, not fan-out, because request
  mutation is order-sensitive. Host-pattern pre-filtered. **Demo/test scope**; not a
  product contract and carries no compatibility commitment.

### 3.4 Sequences

- [ ] `p1` - **ID**: `cpt-cf-genai-security-seq-sync-scan` — capture → scan → verdict → enforce
- [ ] `p1` - **ID**: `cpt-cf-genai-security-seq-deferred-scan` — fast verdict now, deferred backend drains later, same `trace_id`
- [ ] `p1` - **ID**: `cpt-cf-genai-security-seq-attachment-scan` — attachment → `file-parser` → text → fan-out
- [ ] `p1` - **ID**: `cpt-cf-genai-security-seq-proxy-intercept` — *(demo)* CONNECT → MITM → host match → handler chain → rewrite or block

## 4. Requirement Coverage

Which architectural element owns each upstream requirement. This is a statement of design
ownership, not of implementation state. Requirement IDs are defined outside this
repository; see [§1.2](#12-upstream-requirements).

| Upstream requirement | Owned by |
|---|---|
| `cpt-genaigw-fr-inspection-api` | `prompt-scanner` REST surface and `PromptScannerClientV1` |
| `cpt-genaigw-fr-tenant-access` | PEP authorization, `tenant_id` scoping, Scopable entities |
| `cpt-genaigw-fr-verdict` | `VerdictAction`, `Finding`, span-based redaction, `scan_record`, coverage reporting |
| *policy source* | `PolicyProvider` seam; `ConfigPolicyProvider` and external adapters behind it |
| `cpt-genaigw-fr-scanner-extension` | `ScannerBackendClient` and its GTS specification |
| `cpt-genaigw-fr-data-planes` | Configuration-driven deployment shape |
| `cpt-genaigw-fr-disconnected-operation` | `PolicyProvider` seam; out-of-band model provisioning |
| `cpt-genaigw-fr-resilient-service` | Per-backend timeouts, `Defer`, fan-out isolation, coverage reporting |
| `cpt-genaigw-fr-alerting` | Security-event export and operational signals |
| `cpt-genaigw-nfr-performance` | Session replica pool, deterministic-first tiering |

## 5. Constraints

- [ ] `p1` - **ID**: `cpt-cf-genai-security-constraint-no-python`

  No Python anywhere in the pipeline. The GLiNER2 sidecar was removed, not wrapped.

- [ ] `p1` - **ID**: `cpt-cf-genai-security-constraint-gear-platform`

  Standard gear rules apply: regular gears cannot depend on plugin gears; plugins resolve
  peers through `ClientHub`; domain layers stay free of infra imports.

- [ ] `p1` - **ID**: `cpt-cf-genai-security-constraint-no-model-fetch-in-prod`

  `onnx-runner` loads and verifies model files already on disk. `fetch-models.sh` is a
  dev/CI convenience, never invoked at startup or on the request path.

- [ ] `p1` - **ID**: `cpt-cf-genai-security-constraint-demo-not-product`

  `forward-proxy` and its plugins are demo and test infrastructure. They are excluded from
  product packaging, deployment artifacts, production-readiness work, and product
  acceptance criteria. Work on them is justified only by what it proves about the
  platform or enables in testing.
