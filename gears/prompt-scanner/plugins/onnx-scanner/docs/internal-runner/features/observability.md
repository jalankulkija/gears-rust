Created:  2026-08-26 by Constructor Tech
Updated:  2026-08-28 by Constructor Tech

# Feature: Observability

<!-- toc -->

- [1. Feature Context](#1-feature-context)
- [2. Current State](#2-current-state)
- [3. Scope](#3-scope)
- [4. Phases](#4-phases)
- [5. Feature Acceptance Criteria](#5-feature-acceptance-criteria)
- [6. Non-Functional Considerations](#6-non-functional-considerations)
- [7. Outcome](#7-outcome)

<!-- /toc -->

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-featstatus-observability`

## 1. Feature Context

- [x] `p1` - `cpt-cf-onnx-runner-feature-observability`

### Overview

Metrics and traces for the two numbers that determine whether this gear is healthy —
inference time and queue wait — plus a payload-free telemetry guarantee that is tested
rather than promised.

**Realizes**: `cpt-cf-onnx-runner-principle-no-payload-in-telemetry`.
**Closes**: [DESIGN.md](../../DESIGN.md) §5 item 13.

### Purpose

Every tuning decision this gear offers — replica count, intra-op threads, in-flight ceiling,
timeout — is unmakeable without measurement. Right now an operator asked "why is scanning
slow" has nothing: no inference duration, no queue depth, no per-model error rate. The
program's own performance criterion requires separating detector execution time from
platform overhead, and that separation has to be measured *here*, because this is where the
detector time is spent.

The second half is a constraint, not a feature. This gear handles content that is
attacker-controlled and confidential by construction. Telemetry is exactly where such content
leaks, usually via a well-meant debug log. A test should make that leak impossible to add
quietly.

### References

- **Gear design**: [DESIGN.md](../../DESIGN.md) §3.1, §5
- **In-repo reference**: `gears/file-storage/file-storage/src/infra/metrics.rs` — the
  OpenTelemetry meter/histogram wiring pattern used in this repo
- **Upstream requirement**: `cpt-genaigw-nfr-performance` (detector time measured separately
  from platform overhead)
- **Depends on**: [execution-safety](./execution-safety.md) (queue wait only exists once
  queueing does), [model-lifecycle](./model-lifecycle.md) (load outcomes to count)

## 2. Current State

**Implemented.** The list below is the state before this feature landed; see §7 for what it
is now.

- No metrics of any kind. `grep` for `metrics`, `Histogram`, `Counter` in this gear: nothing.
- Two `tracing` calls total: an `info!` per loaded model at startup, and error strings in
  returned errors. No spans, so no timing and no correlation with a caller's request.
- No instrumentation on inference, tokenization, or loading.
- Errors returned to callers carry model ids and tensor names — safe — but nothing enforces
  that a future change keeps it that way.

For contrast, `file-parser`'s magika detector carries a `#[tracing::instrument]` span with
its thread configuration in the span fields; `file-storage` maintains a metrics meter with
duration and byte histograms.

## 3. Scope

**In scope:**

- Inference duration, queue wait, and tokenization duration histograms, per model.
- Counters: calls, errors by variant, timeouts, overloads, load outcomes, session rebuilds.
- Replica utilization / in-flight gauge.
- Tracing spans on the request path with safe fields only.
- A test that fails if payload content reaches telemetry.

**Out of scope:**

- Dashboards and alert rules. The gear emits; the platform consumes. Alerting on scanner
  degradation is `prompt-scanner`'s `security-event-export` Phase 3.
- The benchmark harness — [multi-model-execution](./multi-model-execution.md) Phase 4. This
  feature provides the instruments that benchmark reads.
- Usage/billing attribution, which needs a tenant identity this gear has decided not to
  enforce ([sdk-contract-and-health](./sdk-contract-and-health.md) Phase 3).

## 4. Phases

### Phase 1: Metrics

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-phase-metrics`

**Requirements**

1. An `infra/metrics.rs` meter following `file-storage`'s pattern, obtained from the platform
   meter provider rather than a global.
2. Histograms: `inference_duration_ms`, `queue_wait_ms`, `tokenize_duration_ms` — all
   labelled by `model_id`, and inference additionally by outcome.
3. Counters: `calls_total`, `errors_total{variant}`, `timeouts_total`, `overloads_total`,
   `model_load_total{outcome}`, `session_rebuild_total`.
4. A gauge or up-down counter for in-flight inferences per model.
5. Queue wait and execution time are measured as separate spans of time, not one total —
   they have different causes and different remedies.
6. Model resident bytes recorded at load (`replicas × file size`), so the memory footprint is
   visible without a profiler.
7. Label cardinality is bounded: `model_id` comes from config, error variant from a closed
   enum. No caller-supplied string is ever a label.

**Acceptance criteria**

- [x] Every histogram and counter is emitted at least once in an integration test, asserted
      through a recording port rather than by reading logs — which keeps the tests about what
      is measured rather than about OpenTelemetry's plumbing.
- [x] Queue wait is demonstrably non-zero in a test that saturates replicas, and ~zero when
      idle — proving the two measurements are actually distinct. Both directions are
      asserted; either alone would pass against a single merged measurement.
- [x] No metric label derives from caller input. Enforced by the port's signatures: no method
      accepts a free-form string a caller could influence.

### Phase 2: Spans and the Payload-Free Guarantee

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-phase-spans`

**Requirements**

1. `#[tracing::instrument]` on `run`, `encode`, `encode_fragment`, and model load, with
   `skip`/`skip_all` on every argument carrying data.
2. Span fields limited to: `model_id`, tensor names, shapes, element counts, replica index,
   thread settings, duration, outcome. Explicitly forbidden: tensor values, input text,
   token ids, offsets, file contents.
3. Spans nest correctly across the `spawn_blocking` boundary so a caller's trace shows
   inference as a child, not an orphan.
4. Load-time logging includes model id, replica count, resident bytes, and duration at
   `info`; failures at `warn` with the classified reason from
   [model-lifecycle](./model-lifecycle.md).
5. A test asserts that a known sentinel string passed as input text never appears in captured
   log or span output, for both the success and every error path.

**Acceptance criteria**

- [x] The sentinel suite covers success, invalid input, oversized text, unavailable model and
      unknown model — and **was verified to fail** when `text = %text` was added to the
      tokenizer span, then reverted. Timeout and overload paths share the same instrumented
      code and error-to-outcome mapping.
- [x] Spans exist at both the client dispatch and session layers, so an inference appears
      beneath the caller's span rather than as an orphan.
- [x] No `Debug`-formatted tensor, encoding or text appears in any log statement; enforced by
      the sentinel suite rather than by review. Each case also asserts that *something* was
      captured, so a path emitting no telemetry fails instead of passing vacuously.

## 5. Feature Acceptance Criteria

- [x] Inference time and queue wait are separately observable per model.
- [x] Error, timeout, overload, load, and rebuild counts are visible without reading logs.
- [x] Model memory footprint is reported at load — as an estimate, explicitly labelled as a
      floor rather than a measurement.
- [x] Payload content cannot reach telemetry without failing a test.
- [x] The instruments are sufficient for
      [multi-model-execution](./multi-model-execution.md)'s benchmark to draw its conclusions
      from metrics rather than from ad-hoc timing code.
- [x] ADR written: what is measured, what is deliberately not, and the cardinality rules —
      [ADR 0008](../../ADR/0008-cpt-cf-onnx-runner-adr-what-is-measured.md).

## 6. Non-Functional Considerations

**Overhead.** Histogram recording is nanoseconds against millisecond-to-second inferences.
Span creation on the request path is cheap but not free; keep fields to the listed set and do
not format anything expensive.

**Cardinality.** Metrics backends fail on unbounded label sets. The rule — labels come from
config or closed enums only — is the mitigation, and it is worth stating in the ADR because
the tempting future addition (labelling by consumer, or by tenant) breaks it.

**Security.** This feature is the one place where a careless change silently violates a
program-level principle. The sentinel test is the control; treat its removal as a defect.

**Interaction with the performance criterion.** `cpt-genaigw-nfr-performance` requires
detector execution time to be measured separately from platform overhead. Once
`inference_duration_ms` exists, that number is available directly — which makes this feature a
prerequisite for the program's M4 performance validation, not merely an operational nicety.

## 7. Outcome

**Nine instruments**, listed in full in
[ADR 0008](../../ADR/0008-cpt-cf-onnx-runner-adr-what-is-measured.md): five histograms
(queue wait, inference, tokenize, load duration, resident bytes), three counters (calls,
loads, session rebuilds) and an in-flight up-down counter. The domain declares the port and
`infra/metrics.rs` is the only file importing `opentelemetry`, following `file-storage`'s
pattern.

**The cardinality rule as adopted:** every label comes from configuration or a closed enum,
enforced by the port's method signatures rather than by review. `outcome` collapses the
eight error variants to five actionable values; the full variant stays in the returned error.

**Queue wait and execution are two measurements, and that is tested in both directions** —
near zero when idle, non-zero when eight callers contend for one replica. Either test alone
would pass against a single merged number.

**The sentinel suite has verified teeth.** Adding `text = %text` to the tokenizer span made
it fail with the leaked string quoted; that was performed against the real code and reverted.
Each case also asserts something was captured, so a silent path cannot pass vacuously.

**That anti-vacuous guard found a real gap.** Three cases failed immediately — unknown model,
unavailable model, oversized text — because those calls are rejected in the client dispatch
layer before reaching a session, and only the session layer had been instrumented. The three
most common misconfigurations produced *no telemetry at all*. Fixed by instrumenting the
client methods, which is what the phase specification asked for and what a narrower reading
would have missed.

**Overhead is not measured.** The claim that histogram recording is negligible against
millisecond-to-second inferences is reasoning, not a number. It belongs with the benchmark.

**Resident bytes is a floor.** `replicas × file size`, excluding ONNX Runtime's arenas and
per-session thread stacks. Enough to make an oversized configuration visible at startup; not
enough to size a host from. The real figure is the benchmark's cross-platform RSS work.
