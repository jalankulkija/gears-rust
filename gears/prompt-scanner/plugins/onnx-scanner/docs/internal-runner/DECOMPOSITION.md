Created:  2026-08-24 by Constructor Tech
Updated:  2026-09-04 by Constructor Tech

# Decomposition: onnx-runner

<!-- toc -->

- [1. Overview](#1-overview)
- [2. Capability Assessment](#2-capability-assessment)
- [3. Feature Index](#3-feature-index)
- [4. Shipped Without a Feature File](#4-shipped-without-a-feature-file)
- [5. ADR Backlog](#5-adr-backlog)
- [6. Deliberate Omissions](#6-deliberate-omissions)

<!-- /toc -->

## 1. Overview

`onnx-runner` is a generic ONNX execution gear — tensors and tokenizers, never model
families ([DESIGN.md](../DESIGN.md) §3.1). It began as a **spike** and is well out of it now:
42 tests running in CI against a committed ONNX fixture, `infra/` covered, every call
bounded, a bad model artifact no longer fatal, and the replica pool measured on real hardware
rather than assumed. Features 3.1, 3.3, 3.4 and 3.7 are complete, 3.2 all but warmup, 3.6
half (measurement done, configuration deferred); 3.5 and 3.9 remain.

**Still missing for iteration 1:** operator-visible health, serializable SDK types,
concurrent multi-model verification, the measured defaults in shipped config, and the
documentation set (PRD, README, `GEARS.md` entry, three ADRs). The `execution_provider`
gap is closed by *recording* the CPU-only posture rather than by building the field — see
3.6 and [RELEASE_PLAN §8](./RELEASE_PLAN.md#8-triaged-out-of-iteration-1).

This document is the plan for taking it from spike to a gear that meets the standard of
`file-parser` — which solves the same problem (an ONNX session behind an async trait) with
2,339 lines of integration tests, an ADR, and a documented concurrency model.

**Overall implementation status:**

- [ ] `p1` - **ID**: `cpt-cf-onnx-runner-status-overall`

## 2. Capability Assessment

What a general-purpose ONNX executor in this repo needs, and where we stand. "Standard"
means an existing repo convention or a defect class the repo's own checklists call out, not
an aspiration.

### NEED — release blockers

| Capability | State | Why it blocks |
|---|---|---|
| Inference off the async runtime | ✅ `spawn_blocking` + async mutex; load off-runtime too | Degrades every task in the process, not just this gear. `file-parser` already solved it here. |
| Survives a panicking inference | ✅ slot emptied on unwind, rebuilt on next call | One bad input permanently disables a replica. |
| No panics in the request path | ✅ replica count is `NonZeroUsize`; assert removed | Repo standard: no panics on request paths. |
| Bounded inference (deadline, in-flight cap) | ✅ per-model deadline + in-flight ceiling | Unbounded caller work; no way to shed load. |
| Bounded input (size, tensor count, shape check) | ✅ validated before allocation, typed rejection | Memory exhaustion from a single request. |
| Typed failure per model, not per process | ✅ `Unavailable` + stable reason; gear still starts | Unacceptable blast radius for a shared gear. |
| No silent tensor reinterpretation | ✅ dtype required and verified against the graph at load | Produces plausible wrong numbers — the failure mode the graph check exists to stop. |
| `infra/` under test | ✅ load, verification, inference, dtypes and tokenizer all covered | Session loading, verification, and tokenization are entirely unverified. |
| Integration test against a real model | ✅ real session in CI via a 285-byte fixture; real models still feature-gated | Nothing proves the gear works end to end. |

### NEED — to call it "general purpose"

| Capability | State | Why |
|---|---|---|
| Operator-visible readiness | ✅ `onnx-runner-models` composite check | Zero models reports unhealthy, a partial outage degraded and named. |
| Serializable SDK types | ✅ serde on all seven public types | Unblocks an out-of-process/gRPC transport without committing to one. |
| Real execution-provider config | ◐ CPU hard-coded, **deliberately** | The false documentation is gone. Iteration 1 records CPU-only as a measured decision (ADR 0009) instead of shipping a knob whose non-CPU paths no host here can test. Non-CPU deployment stays impossible, and that is now a stated position rather than an omission. |
| Sound thread/memory model for replicas | ◐ budget measurable, not yet enforced | `intra_threads` is **confirmed honored** — the OpenMP concern died with `download-binaries` (RELEASE_PLAN 5.0). What remains for iteration 1 is cheap: compute and log `replicas × intra_threads`, warn on oversubscription, and ship the measured defaults. `PrepackedWeights` and the global pool are triaged out — peak RSS is now measured per configuration, so the double cost is visible rather than unknown. |
| Defaults set from measurement | ✗ `config/genai-security.yaml` ships `replicas: 1` | A quarter of achievable throughput on a server, against a measured optimum of 12x4. The benchmark's output is inert until the config reflects it. |
| Per-model health / readiness | ✅ one projection serves both the client contract and the platform check | Consumers need `Failed` coverage; operators need readiness. |
| Metrics and spans | ✅ nine instruments; queue wait separated from execution | Inference duration and queue wait are the only numbers that matter for tuning. |
| ADRs + PRD + README + `GEARS.md` entry | ◐ 8 ADRs written; PRD and `GEARS.md` entry still absent | Handover requirement; every mature gear here has them. |

### NICE — real value, not blocking

| Capability | Rationale |
|---|---|
| Batch inference | Largest single throughput win for embedding/classification consumers. |
| Tokenizer truncation/padding policy, `decode`, batch `encode` | Long-input behavior is currently undefined at this layer. |
| Model warmup | Removes first-request latency cliff. |
| Concurrency benchmark **in CI** | The benchmark itself is done and committed (3.6 Phase 4); *running it in CI as a regression guard* is what remains, and it is not a release requirement — the sweep takes minutes per configuration and needs server-class hardware to say anything. |
| Recorded `ort` pre-release posture | Publishing on `=2.0.0-rc.12` is a decision, not an accident. |

### LATER — deliberately deferred

Runtime load/unload/reload; lazy loading; GPU/NPU provider validation; quantization
profiles; OoP/gRPC sidecar mode; model signing and provenance attestation; multi-host
distributed inference.

## 3. Feature Index

Grouped by concern; numbering is grouping, not implementation order.

### 3.1 [Execution Safety](./features/execution-safety.md) ✅ Done

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-feature-execution-safety`
- **Type**: Correctness · **Phases**: 4 of 4 complete · **Depends on**: None
- **Purpose**: Move inference off the async runtime, make it cancel-safe, survive panics,
  and remove every panic from the request path.
- **Realizes**: `cpt-cf-onnx-runner-principle-no-blocking-on-async`
- **Gaps closed**: [DESIGN.md](../DESIGN.md) §5 items 1-3
- **Verified by**: concurrency on a two-worker runtime, unrelated-task progress during
  inference, panic recovery, and mid-flight cancellation — all against a real model.
- **Also landed here**: model loading moved off the async runtime and bounded against the
  `ort` startup hang ([DESIGN.md](../DESIGN.md) §4 decision 7), via `libs/ort-bootstrap`
  shared with `file-parser`; `ort`'s conflicting `download-binaries` feature dropped; and
  `make onnxruntime-fetch` — shared with `file-parser` and CI — to provision a compatible
  runtime.
- **ADR**: [0004 — blocking-pool offload and cancel-safety](../ADR/0004-cpt-cf-onnx-runner-adr-blocking-pool-offload.md).

### 3.2 [Model Lifecycle and Availability](./features/model-lifecycle.md) ◐ Mostly done

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-feature-model-lifecycle`
- **Type**: Reliability · **Phases**: 2 of 3 complete (readiness done, warmup deferred) · **Depends on**: 3.1
- **Purpose**: A model that fails to load is unavailable, not fatal. Per-model state,
  typed errors distinguishing unknown from unavailable, readiness introspection, warmup.
- **Realizes**: `cpt-cf-onnx-runner-seq-degraded-load`
- **Gaps closed**: §5 item 4
- **Cross-gear, still open**: feeds `prompt-scanner`'s coverage model — an unavailable
  model must surface as `Failed` coverage, not as an absence of findings. The producer
  side is done; the consumer-side mapping is `coverage-reporting` Phase 2 and is not.
- **Deferred**: warmup, which needs per-model synthetic inputs or a richer contract than
  tensor names — see that feature file's §7.

### 3.3 [Input Validation and Resource Bounds](./features/input-bounds.md) ✅ Done

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-feature-input-bounds`
- **Type**: Security · **Phases**: 3 of 3 complete · **Depends on**: 3.1
- **Purpose**: Validate shape against data, cap input size and tensor count, enforce an
  inference deadline and an in-flight ceiling, and reject unconfigured output dtypes
  instead of guessing.
- **Realizes**: `cpt-cf-onnx-runner-principle-bounded-work`,
  `cpt-cf-onnx-runner-principle-fail-at-load`
- **Gaps closed**: §5 items 5-7
- **ADR**: [0006 — bounds and defaults](../ADR/0006-cpt-cf-onnx-runner-adr-bounds-and-defaults.md)
- **Breaking**: a config declaring an output without an `output_dtypes` entry no longer
  starts. The one affected config in this repo was already compliant.
- **Still open**: separating queue wait from execution time, which needs
  [observability](./features/observability.md)'s histograms.

### 3.4 [Test Suite](./features/test-suite.md) ✅ Done

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-feature-test-suite`
- **Type**: Quality · **Phases**: 4 of 4 complete · **Depends on**: 3.1 (shape), then tracks each feature
- **Purpose**: Cover `infra/`, add an integration suite against a tiny committed fixture
  model, prove tokenizer offsets against multi-byte input, and wire it into CI without a
  528 MB download.
- **Gaps closed**: §5 item 8
- **ADR**: [0007 — test fixture strategy](../ADR/0007-cpt-cf-onnx-runner-adr-test-fixture-strategy.md)
- **Finding**: tokenizer offsets are **byte** offsets, verified against multi-byte input.
  The documented risk to every consumer's redaction spans is closed, not escalated.
- **Still open**: the concurrency, panic-recovery and cancel-safety suites need the real
  model, so they stay feature-gated; porting them to the fixture brings them into CI.

### 3.5 [SDK Contract and Health](./features/sdk-contract-and-health.md) ✅ Done

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-feature-sdk-contract`
- **Type**: Platform · **Phases**: 3 of 3 complete · **Depends on**: 3.2, 3.3
- **The operability defect is closed.** A gear serving zero models now reports unhealthy
  rather than healthy-by-vacuity, and a partial outage reports degraded naming the model
  and its stable reason code.
- **ADR**: [0011 — the `SecurityContext` position](../ADR/0011-cpt-cf-onnx-runner-adr-security-context-position.md)
  — trusted in-process compute primitive; `ctx` carried for trace propagation, non-enforcement
  stated at the trait.
- **One spelling for three audiences**: `ModelFailureReason` serializes to the same
  `snake_case` code it logs under, which is also what the healthcheck body reports. A test
  pins the equality.
- **Purpose**: serde on SDK types, per-model readiness through the platform `Healthcheck`
  registry, retryable-vs-terminal error classification, and an explicit position on
  `SecurityContext`.
- **Gaps closed**: §5 items 9, 10, 12
- **Not in scope, permanently**: any HTTP surface for this gear
  (`cpt-cf-onnx-runner-constraint-no-http-surface`).

### 3.6 [Execution Configuration](./features/multi-model-execution.md) ◐ Verified and measured; defaults not yet shipped

- [ ] `p1` - **ID**: `cpt-cf-onnx-runner-feature-multi-model`
- **Type**: Platform · **Phases**: 2 of 4 complete (Phases 1 and 4 done, Phase 3 half) ·
  **Est**: 0.75d remaining in iteration 1 · **Depends on**: 3.1, 3.2, 3.7
- **Purpose**: Verify concurrent multi-model hosting, make the replica pool's thread and
  memory cost visible and tunable, and settle its sizing by measurement rather than belief.
- **Phase 4 (benchmark) — done, and it answered the founding question.** At a fixed 48-thread
  budget, **many thin replicas beat one fat session by ~4x** under concurrent load. The pool
  design is confirmed. Harness: `src/bin/benchmark.rs`, four axes, shed calls and peak RSS
  per configuration, reading 3.7's instruments. Results in
  [docs/benchmarks/](../benchmarks/), conclusions in
  [PERFORMANCE-RESEARCH.md](../benchmarks/PERFORMANCE-RESEARCH.md).
- **The four-axis insistence paid off, by being wrong in a useful way.** The plan predicted
  input size would decide replicas-versus-threads. It does not — **concurrency** does: one
  caller always favours threads, four or more always favour replicas, and input size only
  moves the margin. A three-axis sweep would have produced a confident wrong answer.
- **Two findings that redirect effort away from this gear**: the workload is
  memory-bandwidth-bound (best arrangement reaches ~36% of linear scaling; hyperthreads do
  nothing), and **model choice is a 12x lever against ~1.3x for every runtime knob
  combined**. Tuning the runner is not where the wins are.
- **The `intra_threads` gate is cleared.** It is honored; the OpenMP concern died when
  Stage R dropped `download-binaries` for the official dynamic releases, neither of which
  links OpenMP.
- **Phase 2 (`execution_provider`) is deliberately not built.** No non-CPU target exists and
  the workload is bandwidth-bound, so the field would ship untested paths that read as a
  promise. Iteration 1 records the CPU-only posture as ADR 0009 instead. Reopen when a
  deployment target has a GPU.
- **Phase 1 (multi-model hosting) — done, no defect found.** A second committed fixture that
  disagrees with the first on names, arity and dtypes; nine tests covering independent
  contracts and digests, dispatch isolation, the three distinct not-served outcomes, and 24
  interleaved concurrent calls across both models. Two copies of one graph would have proven
  nothing here, which is why the second fixture is the substance of the phase. Real-model
  scale stays Phase 4's question.
- **Remaining for iteration 1**: Phase 3's cheap half (thread budget computed, logged,
  warned), and the measured defaults reaching shipped config.
- **Triaged out**: shared `PrepackedWeights`, the environment-level global thread pool
  (which `ort` says a library must never configure — it belongs to the application), and
  running the benchmark in CI. See
  [RELEASE_PLAN §8](./RELEASE_PLAN.md#8-triaged-out-of-iteration-1).
- **Gaps closed**: §5 item 11; **validates recorded decision 2** — the pool is measured, not
  assumed
- **Note**: Previously scoped as "multi-model execution" and listed as `Partial` on the
  strength of an `execution_provider` field that does not exist. Rescoped accordingly.

### 3.7 [Observability](./features/observability.md) ✅ Done

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-feature-observability`
- **Type**: Operations · **Phases**: 2 of 2 complete · **Depends on**: 3.1, 3.2
- **Purpose**: Inference and queue-wait histograms, per-model counters, tracing spans, and
  a payload-free telemetry guarantee with a test that enforces it.
- **Realizes**: `cpt-cf-onnx-runner-principle-no-payload-in-telemetry`
- **Gaps closed**: §5 item 13
- **ADR**: [0008 — what is measured](../ADR/0008-cpt-cf-onnx-runner-adr-what-is-measured.md)
- **Found on the way**: calls rejected at the client dispatch layer — unknown model,
  unavailable model, oversized text — emitted no telemetry at all. Caught by the sentinel
  suite's anti-vacuous guard, fixed by instrumenting the client methods.
- **Unblocks**: 3.6's benchmark, which reads these instruments rather than timing by hand.

### 3.8 Batch Inference and Tokenizer Policy 🟢 P2

- [ ] `p2` - **ID**: `cpt-cf-onnx-runner-feature-batch-and-tokenizer`
- **Type**: Enhancement · **Phases**: 2 · **Est**: 4d · **Depends on**: 3.5
- **Purpose**: `run_batch`, configured truncation/padding, `decode`, batch `encode`.
- **Gaps closed**: §5 items 14, 15
- **Feature file deferred** until 3.5 fixes the contract shape — writing it now would spec
  against an interface about to change.

### 3.9 Documentation and Decision Records 🟡 P1

- [ ] `p1` - **ID**: `cpt-cf-onnx-runner-feature-docs`
- **Type**: Documentation · **Phases**: — · **Est**: 1.75d remaining · **Depends on**: work it records
- **Purpose**: Three remaining ADRs (§5 items 2, 3, 6), a gear PRD, a real README, an
  `api/README.md`, and a `GEARS.md` entry under Model Runtime Controller.
- **All of it is in iteration 1**, and it is now more than half the remaining work. Two of
  the three ADRs record *measurements already taken* (the pool outcome, the CPU default);
  the third records inherited risk (`ort` on a release candidate, plus the `load-dynamic`
  posture and operator-provisioned dylib that Stage R introduced).
- **Gaps closed**: §5 items 16, 17

## 4. Shipped Without a Feature File

Delivered in the spike. Listed for accounting; corrected against the code, because the
previous version of this table claimed two capabilities that do not exist.

| Capability | Evidence | State |
|---|---|---|
| Graph-contract verification (order-independent) | `domain/model_registry.rs`, 3 tests | Working |
| SHA-256 verification at load, skippable for dev | `domain/model_registry.rs` + `infra/ort_session.rs`, 2 tests | Working |
| Round-robin replica selection | `domain/model_registry.rs`, 3 tests | Working; zero replicas now unrepresentable |
| Session replica pool per model | `infra/ort_session.rs` | Working, unbenchmarked; no longer blocks the runtime |
| Tensor marshalling in / typed extraction out (I64, F32, U8, Bool) | `infra/ort_session.rs` | Untested; silently defaults unconfigured outputs to F32 |
| HuggingFace tokenizer wrapper (`encode`, `encode_fragment`) | `infra/tokenizer.rs` | Untested; offsets unverified against multi-byte input |
| `OnnxRunnerClientV1` in `ClientHub` | `gear.rs`, `onnx-runner-sdk` | Working |
| Pinned-revision model fetch script | `scripts/fetch-models.sh`, `make onnx-models-fetch` | Working |
| ~~`execution_provider` config field~~ | — | **Does not exist.** CPU hard-coded, and now deliberately so — see 3.6. |
| Model `id2label` served as `LoadedModelInfo.labels` | `infra/ort_session.rs`, new `labels_file` config key | Working. Sidecar metadata, served exactly as `tokenizer.json` is; absent or malformed yields `None` and never fails a load, and a non-contiguous map is rejected rather than padded. Names no model family. |
| `declared_input_names()` — a graph's input contract, read the way the gear reads it | `infra/ort_session.rs` | Working. Not a convenience: the declared set is not a property of the file alone, and `piiranha-v1` reports a different set from a plainly-built session than from the gear's. |
| Benchmark harness | `src/bin/benchmark.rs`, `scripts/run-remote-benchmark.sh`, `deploy/docker/benchmark-bundle.Dockerfile` | Working. Belongs to 3.6 Phase 4; listed here because the remote-run tooling around it is not covered by any feature file. |

## 5. ADR Backlog

Items 1-4 correspond to program
[DECOMPOSITION](../../../docs/genai-security/DECOMPOSITION.md) §4 items 6-9.

| # | Decision | State |
|---|---|---|
| 1 | Generic tensor layer, not model-family-aware | ✅ [ADR 0001](../ADR/0001-cpt-cf-onnx-runner-adr-generic-tensor-layer.md) |
| 1a | No HTTP surface: in-process client plus platform healthcheck only | ✅ [ADR 0003](../ADR/0003-cpt-cf-onnx-runner-adr-no-http-surface.md) |
| 4 | Model provisioning out-of-band; no fetching in production | ✅ [ADR 0002](../ADR/0002-cpt-cf-onnx-runner-adr-out-of-band-model-provisioning.md) |
| 7 | Degraded startup, and why `require_all_models` defaults to permissive | ✅ [ADR 0005](../ADR/0005-cpt-cf-onnx-runner-adr-degraded-startup.md) |
| 8 | Bounds, defaults, and the honest limits of timeout semantics | ✅ [ADR 0006](../ADR/0006-cpt-cf-onnx-runner-adr-bounds-and-defaults.md) |
| 9 | Test fixture strategy, and what is deliberately not tested | ✅ [ADR 0007](../ADR/0007-cpt-cf-onnx-runner-adr-test-fixture-strategy.md) |
| 10 | What is measured, what is not, and the cardinality rules | ✅ [ADR 0008](../ADR/0008-cpt-cf-onnx-runner-adr-what-is-measured.md) |
| 2 | Session replica pool over a single shared session, and its thread/memory cost | **Ready to write** — the benchmark settled the sizing (~4x for thin replicas at a fixed budget; 12x4 measured optimum). Iteration 1, step 5.5 |
| 3 | CPU execution provider by default | **Ready to write, and its scope changed** — no longer "CPU by default" but *CPU only, deliberately*, with the bandwidth-bound finding as the evidence. Iteration 1, step 5.5 |
| 5 | Blocking-pool offload and cancel-safety model | ✅ [ADR 0004](../ADR/0004-cpt-cf-onnx-runner-adr-blocking-pool-offload.md) |
| 6 | `ort` pinned to a release candidate; onnxruntime binary distribution | With 3.9, iteration 1. **Scope grew with Stage R**: it must now also cover the `load-dynamic` posture and the operator-provisioned dylib, not just the pin |
| 11 | `SecurityContext` position: trace propagation, not enforcement | ✅ [ADR 0011](../ADR/0011-cpt-cf-onnx-runner-adr-security-context-position.md) — the recommended answer was taken |

## 6. Deliberate Omissions

- **Caller-supplied model paths or bytes.** A security property, not a missing feature
  (`cpt-cf-onnx-runner-constraint-no-caller-paths`). Dynamic loading, if it ever arrives,
  must keep operator-configured registration.
- **Model-family logic.** Belongs in the consumer. A feature that seems to need it here is
  a design failure to escalate.
- **Model fetching in production** (`cpt-cf-onnx-runner-constraint-no-fetch-in-prod`).
- **Model selection and detection quality.** A `prompt-scanner` concern. This gear must not
  acquire an opinion about which model is good.

  **This boundary is currently being crossed, and iteration 1 should uncross it.**
  `docs/benchmarks/QUALITY-EVALUATION.md`, `QUALITY-RESULTS.md` and `docs/benchmarks/quality/`
  score seven candidate PII models for detection accuracy. They landed here because the
  benchmark harness was here, which is a good reason for the work to have happened and a bad
  reason for it to live in this gear's docs. The *performance* numbers are legitimately ours —
  they measure the runner. The quality numbers measure the models, and they belong with the
  consumer that chooses one. Moving them is a tidy, not a debate.
- **An HTTP API of its own.** Standing non-goal, not deferred work
  (`cpt-cf-onnx-runner-constraint-no-http-surface`). This gear is reached in-process through
  `ClientHub`, like `authn-resolver`, `authz-resolver`, `tenant-resolver`, and `cluster`.
  Operator readiness goes through the platform `Healthcheck` registry. If an out-of-process
  consumer ever appears, the answer is the OoP/gRPC pattern, not JSON tensors over HTTP.
- **GPU provider validation.** The config knob is in scope (3.6); validating a specific GPU
  target is not, until a deployment target exists.
- **Training, fine-tuning, ONNX graph editing, non-ONNX formats.**
