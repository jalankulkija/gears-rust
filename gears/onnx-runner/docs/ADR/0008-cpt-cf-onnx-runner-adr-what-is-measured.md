---
status: accepted
date: 2026-08-28
decision-makers: onnx-runner gear maintainers
---

# Measure queue wait and execution separately, and make a payload leak fail a test

**ID**: `cpt-cf-onnx-runner-adr-what-is-measured`

<!-- toc -->

- [Context and Problem Statement](#context-and-problem-statement)
- [Decision Drivers](#decision-drivers)
- [Decision Outcome](#decision-outcome)
  - [The instruments](#the-instruments)
  - [The cardinality rule](#the-cardinality-rule)
  - [Consequences](#consequences)
  - [Confirmation](#confirmation)
- [Considered Options](#considered-options)
  - [One total duration per call](#one-total-duration-per-call)
  - [Queue wait and execution measured separately](#queue-wait-and-execution-measured-separately)
  - [Spans only, no metrics](#spans-only-no-metrics)
- [What is deliberately not measured](#what-is-deliberately-not-measured)
- [What the sentinel test found](#what-the-sentinel-test-found)
- [Traceability](#traceability)

<!-- /toc -->

## Context and Problem Statement

The gear had no metrics at all and two `tracing` calls total. Every tuning knob it
offers — replica count, intra-op threads, in-flight ceiling, timeout — is unmakeable
without measurement, and an operator asked "why is scanning slow" had nothing to look at.

The second half of the problem is a constraint rather than a gap. This gear handles content
that is attacker-controlled and confidential by construction. Telemetry is exactly where such
content leaks, and it leaks by accident: someone adds `text = %text` to a span while
debugging something unrelated, and it survives review because it looks helpful.

## Decision Drivers

* The benchmark this instrumentation exists to serve needs to attribute cost. A number that
  merges queueing with execution cannot tell whether adding replicas would help.
* The upstream performance requirement asks for detector execution time separately from
  platform overhead. That separation has to happen here, because this is where detector time
  is spent.
* Metrics backends fail on unbounded label sets, and this gear's callers supply model ids,
  tensor names and text — all unbounded if used as labels.
* A guarantee that is only documented is not a guarantee. The payload-free property needs a
  mechanical control.
* Metrics must never become a prerequisite for the gear working.

## Decision Outcome

The domain declares a metrics port and exactly one infra module adapts it to OpenTelemetry,
following this repo's existing convention (`file-storage`, `mini-chat`). Queue wait and
execution are recorded as two separate durations. A sentinel test enforces the payload-free
property.

### The instruments

| Instrument | Kind | Labels |
|---|---|---|
| `onnx_runner_queue_wait_ms` | histogram | `model_id` |
| `onnx_runner_inference_duration_ms` | histogram | `model_id`, `outcome` |
| `onnx_runner_tokenize_duration_ms` | histogram | `model_id`, `outcome` |
| `onnx_runner_model_load_duration_ms` | histogram | `model_id`, `outcome` |
| `onnx_runner_model_resident_bytes` | histogram | `model_id` |
| `onnx_runner_calls` | counter | `model_id`, `op`, `outcome` |
| `onnx_runner_model_load` | counter | `model_id`, `outcome` |
| `onnx_runner_session_rebuild` | counter | `model_id`, `result` |
| `onnx_runner_in_flight` | up-down counter | `model_id` |

`outcome` collapses the eight-variant error enum to five values — `ok`, `invalid_input`,
`timeout`, `overloaded`, `failed` — because an operator watching a dashboard wants to know
which *kind* of thing is going wrong, and those are the five actionable answers. The full
variant remains in the returned error for the caller.

`model_resident_bytes` is `replicas × file size`: **a floor, not a measurement**. It excludes
ONNX Runtime's arenas, per-session thread stacks and pre-packed weights. It is recorded
anyway because it makes "4 replicas of a 446 MB model" visible in a startup log rather than
in an out-of-memory kill, and because the *real* number needs the benchmark's cross-platform
RSS work.

### The cardinality rule

**Every label comes from configuration or a closed enum. No caller-supplied string is ever a
label.** `model_id` comes from the operator's config; `outcome`, `result` and `op` come from
closed enums.

The method signatures on the port are the enforcement: nothing accepts a free-form string a
caller could influence. The tempting future additions — labelling by consumer, by tenant, by
tensor name — all break this, and each would be a decision to revisit here rather than a
detail to add.

### Consequences

* The domain owns the port; `infra/metrics.rs` is the only file importing `opentelemetry`.
  Execution paths never reach for ambient state.
* A `NoopMetrics` implementation is the default, so metrics are never a prerequisite for
  constructing anything, and tests that are not about metrics need no meter. There is a test
  asserting the gear works without a backend.
* The in-flight gauge is decremented by a guard on drop, so a cancelled call cannot leave it
  permanently elevated. A gauge that drifts is worse than no gauge.
* **The client dispatch layer had to be instrumented too**, not just the session layer — see
  below.
* Rejected calls are logged at `debug`, not `warn`. An unknown model id or an oversized input
  is a caller-side mistake, and a misbehaving consumer must not be able to fill an operator's
  logs. The *rate* of those belongs in the counters.
* Tokenization is counted under `op = "encode"` and never as inference, so the two cannot be
  conflated in a dashboard.

### Confirmation

Nine tests through a recording port — asserting on *what is measured* rather than on
OpenTelemetry's plumbing — plus seven sentinel tests.

The load-bearing pair proves the two durations are genuinely distinct measurements rather
than one number recorded twice: queue wait is near zero on an idle gear, and non-zero when
eight callers contend for one replica. Either alone would pass with a single merged
measurement; both together cannot.

The sentinel suite's teeth were verified against the real code, not asserted: adding
`text = %text` to the tokenizer span made it fail with the leaked string quoted in the
failure message. That check was performed and reverted.

## Considered Options

### One total duration per call

Record wall-clock from call to return.

* Good, because it is one instrument and trivially correct
* Good, because it is what a caller experiences
* Bad, because it cannot answer the only question worth asking: a slow call is either
  queueing or executing, and those have opposite remedies — more replicas versus a bigger
  thread budget or a smaller input
* Bad, because the benchmark could not attribute cost, making this feature's main consumer
  useless
* Bad, because the upstream performance requirement explicitly asks for detector time
  separately from platform overhead

### Queue wait and execution measured separately

Chosen. Time permit acquisition and inference independently.

* Good, because it distinguishes contention from cost, which is the actual diagnostic
* Good, because the sum is still available by addition, so nothing is lost
* Good, because it makes the replica-pool question measurable — the whole point of varying
  replica count is to move work between the two
* Neutral, because it needs two histograms rather than one
* Bad, because "total latency" now requires adding two numbers, which a dashboard author
  must know to do

### Spans only, no metrics

Rely on tracing spans and let a backend aggregate.

* Good, because spans carry per-call context that metrics cannot
* Good, because it is less code
* Bad, because sampled traces cannot produce reliable rates or percentiles, and the
  benchmark needs exactly those
* Bad, because it makes measurement depend on a trace pipeline being configured, where a
  counter works with any metrics backend
* Neutral, because both were done: spans for correlation, metrics for aggregation

## What is deliberately not measured

* **Per-tenant or per-consumer attribution.** It would break the cardinality rule, and this
  gear has decided not to enforce tenant identity.
* **True resident memory.** The estimate is a floor; the real figure needs cross-platform RSS
  measurement and belongs with the benchmark.
* **Dashboards and alert rules.** The gear emits; the platform consumes.
* **Instrumentation overhead.** Histogram recording is nanoseconds against
  millisecond-to-second inferences. Stated as reasoning, not measured — and worth stating
  plainly rather than implying a number exists.

## What the sentinel test found

The suite includes a guard against passing vacuously: each case asserts that *something* was
captured, so a path that emits no telemetry fails rather than trivially passing.

That guard immediately failed three cases — unknown model, unavailable model, and oversized
text. Not a test bug: those calls are rejected in the client dispatch layer before reaching a
session, and only the session layer had been instrumented. **The three most common
misconfigurations produced no telemetry whatsoever.** An operator debugging "my scans are
failing" would have found an empty trace.

The fix was to instrument `run`, `encode` and `encode_fragment` on the client, which is what the
feature specification asked for and what a narrower reading of "instrument the inference
path" would have missed. Worth recording because the anti-vacuous assertion — three lines
that felt like defensive padding — is what caught it.

## Traceability

- **DESIGN**: [DESIGN.md](../DESIGN.md) §3.1, §5 item 13

This decision directly addresses the following requirements or design elements:

* `cpt-cf-onnx-runner-principle-no-payload-in-telemetry` — the sentinel suite is this
  principle's mechanical control; treat its removal as a defect
* `cpt-cf-onnx-runner-phase-metrics` — the instrument set and the cardinality rule
* `cpt-cf-onnx-runner-phase-spans` — spans on the request path, with payload-free fields
* `cpt-genaigw-nfr-performance` — detector execution time is now separable from platform
  overhead, which is what the upstream requirement asks for
* `cpt-cf-onnx-runner-phase-benchmark` — the instruments this decision provides are what the
  benchmark reads instead of ad-hoc timing code
