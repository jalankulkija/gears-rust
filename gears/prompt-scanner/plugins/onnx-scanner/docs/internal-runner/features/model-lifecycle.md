Created:  2026-08-26 by Constructor Tech
Updated:  2026-08-27 by Constructor Tech

# Feature: Model Lifecycle and Availability

<!-- toc -->

- [1. Feature Context](#1-feature-context)
- [2. Current State](#2-current-state)
- [3. Scope](#3-scope)
- [4. Phases](#4-phases)
- [5. Feature Acceptance Criteria](#5-feature-acceptance-criteria)
- [6. Non-Functional Considerations](#6-non-functional-considerations)
- [7. Outcome](#7-outcome)

<!-- /toc -->

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-featstatus-model-lifecycle` — Phases 1-2 complete;
      Phase 3 partial (readiness done, warmup deferred; see §7)

## 1. Feature Context

- [x] `p1` - `cpt-cf-onnx-runner-feature-model-lifecycle`

### Overview

A model that fails to load must be *unavailable*, not fatal. Today one bad artifact aborts
gear init, which aborts server startup, which takes down every other gear in the process.

**Realizes**: `cpt-cf-onnx-runner-seq-degraded-load`.
**Closes**: [DESIGN.md](../../DESIGN.md) §5 item 4.

### Purpose

Two audiences need this, for different reasons.

*Operators*: a corrupted download, a wrong checksum after a model bump, or a missing
tokenizer file currently means the whole service will not start, with a message about one
model buried in startup logs. The correct behavior is to start, serve every model that
loaded, and report the one that did not.

*Consumers*: `prompt-scanner` must distinguish "scanned and found nothing" from "the ML
backend was unavailable" — its coverage model depends on it
(`cpt-genaigw-fr-resilient-service` ¶4, via `onnx-scanner`). A gear that either works
entirely or does not start at all gives the consumer no way to report degraded coverage,
because there is no running service to ask.

### References

- **Gear design**: [DESIGN.md](../../DESIGN.md) §2.3, §3.4
- **Consumer dependency**: `gears/prompt-scanner/docs/features/coverage-reporting.md`
- **Program**: [program DESIGN](../../../../docs/genai-security/DESIGN.md) §5 ¶1
- **Depends on**: [execution-safety](./execution-safety.md) — session rebuild-after-panic is
  the same state machine as load-failure recovery

## 2. Current State

**Phases 1-2 implemented; Phase 3 partial.** What follows describes the code as it stood
before this feature landed; the acceptance criteria below are written against it.

Previously, `Gear::init` looped over configured models, calls `LoadedModel::load` and
`LoadedTokenizer::load`, and returns `anyhow::Error` on the first failure. Consequences:

- Any failure — unreadable file, checksum mismatch, contract mismatch, malformed
  `tokenizer.json` — is fatal to the process.
- There is no per-model state anywhere; a model is either in the `BTreeMap` or the gear does
  not exist.
- `list_models` reports what loaded, with no notion of what *should* have loaded.
- `OnnxRunnerError::UnknownModel` conflates "no such model configured" with "configured but
  unavailable" — the caller cannot tell a typo from an outage.
- All replicas load eagerly and serially at startup: 4 replicas of the 441 MB `gliner2-pii`
  is ~1.7 GB of RSS and a slow, silent startup.
- No warmup: the first real request pays first-inference cost (graph optimization, lazy
  allocation), which for a large quantized model is seconds.

## 3. Scope

**In scope:**

- Per-model load outcome as explicit state.
- Non-fatal load failure, with a configurable strictness escape hatch.
- Typed distinction between unknown and unavailable.
- Readiness introspection over the client contract.
- Optional warmup inference per model.

**Out of scope:**

- Runtime load/unload/reload of models — deliberately deferred
  ([DECOMPOSITION.md](../DECOMPOSITION.md) §6). This feature must not preclude it: the state
  machine it introduces is the thing a future reload would drive.
- The operator-facing projection of health —
  [sdk-contract-and-health](./sdk-contract-and-health.md) Phase 2.
- Metrics for load outcomes — [observability](./observability.md).

## 4. Phases

### Phase 1: Per-Model State

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-phase-model-state`

**Requirements**

1. A `ModelState` enum: `Ready`, `Unavailable { reason: ModelFailureReason }`, and (for
   warmup/rebuild windows) `Loading`.
2. `ModelFailureReason` is a stable machine-readable classification —
   `FileUnreadable`, `ChecksumMismatch`, `ContractMismatch`, `TokenizerInvalid`,
   `SessionBuildFailed` — never a free-form string, and never containing payload data.
3. The gear holds every *configured* model, each with its state, rather than only the ones
   that loaded.
4. `ModelInfo` gains state; `list_models` reports configured models including unavailable
   ones.
5. A model whose sessions were all lost to panics (per
   [execution-safety](./execution-safety.md) Phase 2) transitions to `Unavailable` with
   `SessionBuildFailed` rather than failing every call silently.

**Acceptance criteria**

- [x] `list_models` returns an entry for a model whose file is missing, marked
      `Unavailable { FileUnreadable }`.
- [x] Each `ModelFailureReason` variant is produced by a test that induces that failure
      (bad checksum, renamed tensor, corrupt tokenizer, unreadable file, zero replicas).
- [x] No reason string contains a path outside the configured model dir, or any tensor
      data — `ModelFailureReason` is a fieldless enum, so this holds by construction
      rather than by review. Detail goes to the operator's log, never into the type.

### Phase 2: Non-Fatal Startup

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-phase-degraded-startup`

**Requirements**

1. A model that fails to load is recorded `Unavailable`; `Gear::init` succeeds.
2. Each failure is logged once at `warn` with model id and reason classification.
3. `run`/`encode` against an unavailable model return
   `OnnxRunnerError::ModelUnavailable { model_id, reason }`, distinct from `UnknownModel`.
4. A config flag — `require_all_models` (default `false`) — restores fail-fast startup for
   deployments that prefer it. The default is chosen deliberately and recorded in the ADR.
5. Zero successfully loaded models is not in itself a startup failure when
   `require_all_models` is false, but it is logged at `error`.

**Acceptance criteria**

- [x] A gear configured with one good and one bad model starts, serves the good one, and
      reports the bad one as unavailable.
- [x] `require_all_models: true` with the same config fails startup with a message naming
      the model and reason.
- [x] `ModelUnavailable` and `UnknownModel` are distinguishable by a consumer; asserted in a
      test that a typo'd id and a broken model produce different variants.

### Phase 3: Readiness and Warmup

- [ ] `p1` - **ID**: `cpt-cf-onnx-runner-phase-readiness-warmup`

**Requirements**

1. Readiness is derivable from the in-process client contract: a method or `ModelInfo`
   field answering "is this model ready" and "how many replicas are live." Its
   operator-facing projection is the platform healthcheck in
   [sdk-contract-and-health](./sdk-contract-and-health.md) Phase 2.
2. Optional per-model `warmup: bool` (default off) runs one synthetic inference per replica
   at load, using shapes derived from the declared contract, before the model is marked
   `Ready`.
3. Warmup failure marks the model `Unavailable` — it is a real signal, not a nicety.
4. Warmup never uses caller data and never logs its inputs.
5. Startup remains bounded: warmup happens on the blocking pool and its total cost is logged
   per model.

**Acceptance criteria**

- [x] Readiness reflects reality after a model is lost to repeated session-build failure —
      `ModelInfo::state` and the live replica count are read at call time, not frozen at
      load, and a model that loses every replica transitions to
      `Unavailable { SessionBuildFailed }`.
- [ ] With `warmup: true`, the first client request shows no measurable first-inference
      penalty relative to the second. **Deferred — see §7.**
- [ ] A model whose warmup fails is `Unavailable`, and the gear still starts. **Deferred.**

## 5. Feature Acceptance Criteria

- [x] One bad model artifact never prevents the process from starting (default config).
- [x] Every configured model is observable with a state and, on failure, a stable reason
      code.
- [x] Consumers can distinguish unknown, unavailable, and inference-failed — the three
      cases that map to different coverage outcomes upstream.
- [ ] `onnx-scanner` reports an unavailable model as `Failed` coverage rather than as no
      findings. **Cross-gear, still open**: the producer side is done — this gear now
      returns `ModelUnavailable` with a stable reason instead of failing opaquely — but
      the consumer-side mapping is `prompt-scanner`'s `coverage-reporting` Phase 2 and has
      not been written. Until it is, an unavailable model still reaches a scan verdict as
      an absence of findings, which is exactly the defect the program's largest gap is
      about.
- [x] ADR written: degraded startup, and why the default is permissive —
      [ADR 0005](../../ADR/0005-cpt-cf-onnx-runner-adr-degraded-startup.md).

## 6. Non-Functional Considerations

**Memory.** Per-model `replicas × model size` is the dominant cost and is currently
invisible. Log resident model bytes per model at load; a 441 MB model at the default 4
replicas is a footgun that should be visible in startup logs, not discovered in production.

**Startup time.** Serial loading of N replicas of a large model is slow. Loading replicas
concurrently on the blocking pool is an obvious improvement but interacts with peak memory;
measure before choosing, and record it.

**Security.** Failure reasons are classifications, not diagnostics: no absolute paths beyond
the configured model dir, no partial file contents, no tensor data
(`cpt-cf-onnx-runner-principle-no-payload-in-telemetry`).

**Reliability.** The state machine must be monotonic under concurrency — two callers racing
to rebuild the same session must not produce two sessions or two state transitions.

## 7. Outcome

**The state machine.** Every *configured* model is held as a `ModelEntry` — either `Ready`
with its session pool and tokenizer, or `Unavailable` with a `ModelFailureReason`. The
reason is a fieldless enum (`FileUnreadable`, `ChecksumMismatch`, `ContractMismatch`,
`TokenizerInvalid`, `SessionBuildFailed`, `InvalidConfig`), classified at the point the
cause is known, so the payload-free guarantee holds by construction rather than by review.
Detail reaches the operator's log; the type carries only the code.

`ModelInfo` gained `state` and moved the load-only fields into
`loaded: Option<LoadedModelInfo>` — separate rather than empty-when-unavailable, so a
consumer cannot mistake "declares no outputs" for "never loaded". State and live replica
count are read at call time, so a model that loses every replica to panicking inferences
and fails to rebuild transitions to `Unavailable { SessionBuildFailed }` and says so. That
transition is one-way: nothing in this gear can tell whether the underlying cause was
fixed, so recovery is a restart, or the reload this design defers.

**The strictness default is permissive** (`require_all_models: false`), for a reason
specific to this gear rather than a general preference for leniency: it shares a process
with every other gear, so a fatal load turns one corrupted download into a total outage of
unrelated functionality. That is only safe because the failure is *reported* — consumers
get `ModelUnavailable` with a stable reason, distinct from `UnknownModel`. The permissive
default and the typed error are one decision, not two; shipping the default without the
error would trade a loud failure for a silent one.

**Warmup is deferred, deliberately.** Phase 3 requires synthetic inference at load with
shapes "derived from the declared contract", and requires warmup failure to mark the model
`Unavailable`. The config declares tensor *names* only, and the graph's declared dims are
frequently symbolic. For a model like `gliner2-pii` — four interdependent inputs where
`text_word_indices` and `schema_special_indices` index into `input_ids` — synthetic values
that satisfy the shapes can still be semantically out of range, and the model would fail an
inference it would never fail in production. With warmup failure marked fatal to the model,
that takes a *working* model out of service. Shipping that would be worse than the latency
cliff it removes. It needs either per-model warmup inputs in config, or a contract richer
than names; both are decisions, not implementation. `warmup` stays off and unimplemented
until one is taken.

**Memory and startup numbers are not measured.** §6 asks for resident model bytes per model
at load and a decision on concurrent replica loading; neither is done, and both belong with
the benchmark harness in [multi-model-execution](./multi-model-execution.md).

**A known flake.** Loading and dropping several large sessions in one process occasionally
aborts at *exit* — `libc++abi: recursive_mutex lock failed` — after every assertion has
passed. It is a race in ONNX Runtime's own teardown, the same class `file-parser` records
(ORT's `cpuinfo` asserts if teardown races init). Seen once in five runs of the lifecycle
suite; not reproducible with fewer tests in the process. It does not affect correctness,
but it can fail CI, and the fixture model from
[test-suite](./test-suite.md) is what would make it rare enough to ignore.
