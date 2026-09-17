---
status: accepted
date: 2026-08-27
decision-makers: onnx-runner gear maintainers
---

# Bound every call, and default to limits that are safe for a caller we do not trust

**ID**: `cpt-cf-onnx-runner-adr-bounds-and-defaults`

<!-- toc -->

- [Context and Problem Statement](#context-and-problem-statement)
- [Decision Drivers](#decision-drivers)
- [Considered Options](#considered-options)
- [Decision Outcome](#decision-outcome)
  - [The defaults](#the-defaults)
  - [Consequences](#consequences)
  - [Confirmation](#confirmation)
- [Pros and Cons of the Options](#pros-and-cons-of-the-options)
  - [Queue with a bounded wait](#queue-with-a-bounded-wait)
  - [Reject immediately at capacity](#reject-immediately-at-capacity)
  - [No in-flight ceiling](#no-in-flight-ceiling)
- [On the honesty of the timeout](#on-the-honesty-of-the-timeout)
- [More Information](#more-information)
- [Traceability](#traceability)

<!-- /toc -->

## Context and Problem Statement

The gear trusted its callers completely. No cap on tensor size, tensor count, or text
length; no inference deadline; no limit on concurrent inferences; a declared shape was never
checked against the data it carried. And an output tensor with no configured dtype was
extracted as F32 regardless of what the graph actually emitted.

In-process that is *almost* defensible, since consumers are other gears. It stops being
defensible for three reasons: the content being tokenized is attacker-controlled by
construction — that is the entire point of the scanning program — the gear may one day be
reached out of process, and "trusted caller" is not a property that survives reuse by code
nobody has written yet.

[ADR 0004](./0004-cpt-cf-onnx-runner-adr-blocking-pool-offload.md) sharpened the urgency. It
moved inference onto Tokio's blocking pool, which is **process-wide**: unbounded concurrent
inferences now queue on a resource other gears share. That ADR recorded the in-flight cap as
the missing half of its own decision. This is that half.

The `output_dtypes` default deserves separate mention, because it is a correctness bug and
not a hardening gap. If a model emits I64 for an output declared nowhere, the caller
received F32 — silently, catastrophically wrong numbers rather than an error. That is
precisely the "transposed-axis" failure the graph-contract check exists to prevent, and it
contradicted the gear's own stated philosophy of knowing the model ahead of time and failing
loudly.

## Decision Drivers

* Defaults are what people actually deploy. A limit that only exists when configured is not
  a limit.
* The blocking pool is shared. Unbounded work here degrades gears that have nothing to do
  with inference.
* Validation must happen before allocation, or it is not protection — checking a tensor
  after building it defeats the purpose.
* Rejection must be *typed*, because consumers map outcomes onto coverage states, and
  "something went wrong" is not a coverage state.
* ONNX Runtime cannot cancel an inference mid-graph. Any timeout story has to be honest
  about that rather than implying work stops.
* Today's single consumer must never hit a default. A bound that fires in normal operation
  is a bug report, not a safeguard.

## Considered Options

For load shedding, once a ceiling exists:

* Queue with a bounded wait
* Reject immediately at capacity
* No in-flight ceiling at all

## Decision Outcome

Every call is validated before work starts, bounded by a deadline, and capped in flight.
Output dtypes must be declared for every output and are verified against the graph at load.

`on_overload` defaults to **queue with a bounded wait**: a caller waits for a permit for at
most `inference_timeout`, then receives `Overloaded`. `Reject` is available for deployments
that prefer to shed immediately.

### The defaults

| Setting | Default | Reasoning |
|---|---|---|
| `max_tensor_elements` | 16Mi | 64 MiB as f32, 128 MiB as i64 — far above any real tokenized batch, far below "exhaust the host" |
| `max_input_tensors` | 32 | Generous for any model this gear plausibly hosts; the current one declares four |
| `max_dimension` | 1Mi | Catches a transposed or garbage dimension without constraining a legitimate sequence length |
| `max_text_bytes` | 1 MiB | Well past any prompt. A document belongs in an attachment path that chunks it, not one `encode` call |
| `inference_timeout` | 30s | Generous enough that a large model on a cold CPU is not cut off, tight enough to shed a pathological input |
| `max_in_flight` | replica count | More in flight than there are replicas is queueing, not parallelism |
| `on_overload` | `Queue` | See below |

These are per-model and overridable. They are deliberately loose: the goal is a ceiling that
an attacker hits and a consumer never does, not a tuned quota. Tightening them belongs after
observability makes real latencies and sizes visible —
tuning a limit without data produces a limit that fires on legitimate traffic.

### Consequences

* Four new error variants — `InvalidInput`, `Timeout`, `Overloaded`, plus the earlier
  `ModelUnavailable` — and `OnnxRunnerError::is_retryable` so consumers branch on one method
  instead of matching every variant. The outcome-to-coverage mapping is written down once,
  in input validation and resource bounds §5.
* **Phase 3 is a breaking config change.** YAML that lists an output in `expected_outputs`
  without a matching `output_dtypes` entry now fails startup. Exactly one config in the repo
  is affected and it was already compliant. This is a deliberate break: the alternative is
  keeping a default that silently reinterprets tensors.
* Config completeness is checked *before* any I/O, so an unusable config fails immediately
  rather than after reading a several-hundred-megabyte model file.
* Dtype declarations are now verified against the session's own output metadata at load. A
  model declaring `f32` where the graph emits `i64` fails at startup instead of at first
  inference.
* All three `#[allow(clippy::cast_*)]` attributes on the request path are gone, replaced by
  checked conversions. The limits that would have justified them are operator-set, so the
  conversions prove themselves rather than trusting configuration.
* `ModelConfig` gained a `Default` impl mirroring the serde defaults, so tests and
  programmatic callers set what they care about instead of restating every bound.
* Queue wait and execution time are **not** measured separately, which the feature asked
  for. The deadline measures total elapsed. Separating them needs histograms, which belong
  to observability.
* Validation overhead is unmeasured. The claim that it is noise — O(rank) integer arithmetic
  and one semaphore acquire against millisecond-to-second inferences — is argued, not
  benchmarked.

### Confirmation

Eight pure unit tests over the validation arithmetic, with no model or runtime needed:
shape-versus-data agreement, overflow as a rejection rather than a wrap, zero and oversized
dimensions, element ceilings, empty shapes, and contract name checking in both directions.

Five integration tests against a real model: malformed shape rejected typed, an undeclared
input name reported as a contract error citing the offender, oversized text rejected while
text under the limit still works, an impossible deadline producing `Timeout` with the model
still serving afterwards, and a ceiling of 1 under 8 concurrent calls shedding load.

Config-level failures are covered by the model-lifecycle suite, which now includes an
`undeclared-dtype` case classified `InvalidConfig`.

## Pros and Cons of the Options

### Queue with a bounded wait

Chosen. Wait for a permit up to `inference_timeout`, then `Overloaded`.

* Good, because a consumer's own deadline is usually a better place to give up than an
  arbitrary queue depth chosen here
* Good, because the wait is already bounded by a limit the operator set for this model, so
  it introduces no new knob and no new unbounded thing
* Good, because brief bursts — the common case — are absorbed rather than converted into
  errors a consumer must handle
* Neutral, because it makes `inference_timeout` do double duty, bounding both queueing and
  execution; that is worth stating plainly since a queued call can therefore be rejected
  having done no work at all
* Bad, because under sustained overload callers wait before failing, which is worse than
  failing fast for a caller that had somewhere else to go

### Reject immediately at capacity

Available as `on_overload: reject`.

* Good, because it sheds load at the earliest possible moment, with no wasted waiting
* Good, because it makes overload immediately visible rather than showing up as latency
* Bad, because it converts brief, ordinary bursts into errors — with a ceiling equal to the
  replica count, two concurrent callers on a single-replica model would see failures during
  normal operation
* Bad, because a consumer that retries on `Overloaded` turns rejection into a busy loop,
  which is worse for the pool than a bounded wait

### No in-flight ceiling

Rely on the replica mutexes to serialize.

* Good, because it is what the code already did, and needs nothing
* Bad, because after [ADR 0004](./0004-cpt-cf-onnx-runner-adr-blocking-pool-offload.md) the
  queue is on a process-wide blocking pool: unbounded callers occupy threads other gears
  need, so "no limit here" exports the problem
* Bad, because there is no way to shed load, only to accumulate it
* Bad, because it leaves the gap that ADR explicitly recorded as owed

## On the honesty of the timeout

`Timeout` means **the result was abandoned, not that the work stopped.** ONNX Runtime has no
mid-graph cancellation, so after a deadline expires the replica remains occupied until the
underlying inference finishes on its own.

This is stated on the `run` doc comment, on `OnnxRunnerError::Timeout`, and on the
`inference_timeout` config field, because the consequences are counter-intuitive and
consumer-visible:

* A caller that retries immediately competes with work that is still running.
* Under repeated timeouts a model can be fully occupied by inferences nobody is waiting for,
  which looks like a stall rather than a series of failures.
* The in-flight ceiling is therefore the *effective* protection against pathological input;
  the timeout bounds what a caller waits for, not what the gear does.

Papering over this — presenting `Timeout` as though the work were cancelled — would have
been the easier documentation and the wrong one. If `ort` ever gains real cancellation, this
section is what should be revisited.

## More Information

The tests for this feature produced an unplanned confirmation of a decision recorded
elsewhere. Constructing valid synthetic inputs for `gliner2-pii` took several attempts
against ONNX Runtime's rank complaints: `text_word_indices` wants rank 1 where the obvious
guess is rank 2. Warmup, deferred in
model lifecycle and availability §7, was specified to generate synthetic
inputs from a contract that declares only tensor *names* — and to mark a model unavailable
when they fail. It would have failed in exactly this way, taking a working model out of
service. The deferral was argued from the shape of the problem; this is the evidence.

## Traceability

- **DESIGN**: [DESIGN.md](../DESIGN.md) §3.1, §3.5, §5 items 5-7

This decision directly addresses the following requirements or design elements:

* `cpt-cf-onnx-runner-principle-bounded-work` — this ADR is that principle's realization,
  and closes the gap [ADR 0004](./0004-cpt-cf-onnx-runner-adr-blocking-pool-offload.md) left
* `cpt-cf-onnx-runner-principle-fail-at-load` — restored for output dtypes, which previously
  violated it by defaulting at runtime
* `cpt-cf-onnx-runner-phase-input-validation` — shape, name, size, and overflow checking
* `cpt-cf-onnx-runner-phase-deadlines` — the deadline, the ceiling, and the shedding policy
* `cpt-cf-onnx-runner-phase-dtype-strictness` — declaration required, verified against the
  graph at load
* `cpt-cf-onnx-runner-contract-model-config` — the config surface this decision extends
