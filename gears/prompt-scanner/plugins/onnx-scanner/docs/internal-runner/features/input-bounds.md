Created:  2026-08-26 by Constructor Tech
Updated:  2026-08-27 by Constructor Tech

# Feature: Input Validation and Resource Bounds

<!-- toc -->

- [1. Feature Context](#1-feature-context)
- [2. Current State](#2-current-state)
- [3. Scope](#3-scope)
- [4. Phases](#4-phases)
- [5. Feature Acceptance Criteria](#5-feature-acceptance-criteria)
- [6. Non-Functional Considerations](#6-non-functional-considerations)
- [7. Outcome](#7-outcome)

<!-- /toc -->

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-featstatus-input-bounds`

## 1. Feature Context

- [x] `p1` - `cpt-cf-onnx-runner-feature-input-bounds`

### Overview

Every call becomes bounded and every input validated: shape consistency, size caps,
inference deadline, in-flight ceiling, and no silent reinterpretation of output tensors.

**Realizes**: `cpt-cf-onnx-runner-principle-bounded-work`,
`cpt-cf-onnx-runner-principle-fail-at-load`.
**Closes**: [DESIGN.md](../../DESIGN.md) §5 items 5-7.

### Purpose

The gear currently trusts its callers completely. In-process that is *almost* defensible —
consumers are other gears. It stops being defensible for three reasons: the content being
tokenized is attacker-controlled by construction (that is the whole point of the scanning
program), the gear may one day be reached out-of-process, and "trusted caller" is not a
property that survives the gear being reused by code we have not written yet.

The `output_dtypes` default deserves separate mention because it is a *correctness* bug, not
just a hardening gap. An output tensor whose dtype is not configured is extracted as F32. If
the model actually emits I64, the caller receives numbers that are silently, catastrophically
wrong rather than an error — precisely the "transposed-axis" class of failure the
graph-contract check was added to prevent. The gear's own stated philosophy is "know your
model ahead of time, fail loudly on mismatch"; this default violates it.

### References

- **Gear design**: [DESIGN.md](../../DESIGN.md) §3.1, §3.5, §5
- **Depends on**: [execution-safety](./execution-safety.md) — the deadline and the in-flight
  cap both attach to the queueing primitive introduced there
- **Related**: `gears/file-parser` size and timeout handling, for the established shape of
  per-request limits in this repo

## 2. Current State

**Implemented.** What follows describes the code as it stood before this feature landed;
the acceptance criteria below are written against it.

Previously:

- `NamedTensor { name, shape, data }` was accepted as given. `shape` is never checked against
  `data.len()`; a mismatch becomes an opaque `Internal("building I64 tensor ...")` from
  `Tensor::from_array`, or worse, a valid-but-wrong tensor.
- No cap on tensor element count, byte size, tensor count per call, or text length for
  `encode`. A caller can request an arbitrarily large allocation.
- No inference deadline. A pathological input occupies a replica for as long as ONNX Runtime
  takes; there is no cancellation and no way to shed it.
- No in-flight ceiling. Concurrent calls queue on replicas without bound, and after
  [execution-safety](./execution-safety.md) they will queue on the shared blocking pool —
  which other gears also use.
- `output_dtypes.get(name).unwrap_or(OutputDtype::F32)` — silent reinterpretation, as above.
- Input tensor names are not validated against the declared contract at call time: a caller
  can pass a tensor the model does not expect and get an `ort`-level error rather than a
  contract error.
- Casts (`usize`→`i64` for shapes, `i64`→`usize` on extraction) carry `#[allow]` attributes
  justified by comments asserting the values are small. That is true for today's one
  consumer and unchecked in general.

## 3. Scope

**In scope:**

- Shape/data consistency validation and input-name contract checking at call time.
- Configurable per-model input limits with documented defaults.
- Per-call inference deadline with a typed timeout outcome.
- Bounded in-flight inference count.
- Removal of the `output_dtypes` default; dtype completeness enforced at load.
- Audit of numeric casts on the request path.

**Out of scope:**

- Authorization and per-tenant quota — [sdk-contract-and-health](./sdk-contract-and-health.md)
  decides the `SecurityContext` position.
- Batching, which changes the shape of "one call" — deferred to
  [DECOMPOSITION.md](../DECOMPOSITION.md) §3.8.
- Tokenizer truncation policy — same deferred feature; this feature only *rejects* oversized
  text rather than silently truncating it.

## 4. Phases

### Phase 1: Input Validation

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-phase-input-validation`

**Requirements**

1. For every input tensor, `shape.iter().product() == data.len()`; violation returns a typed
   `InvalidInput { model_id, tensor, message }` before any allocation.
2. Rank and per-dimension sanity: no zero-length dimension unless the model's contract
   permits it, no dimension exceeding a configured maximum.
3. Input tensor names are checked against the model's declared `expected_inputs`: unknown
   names and missing required names both fail typed, before `Session::run`.
4. `encode`/`encode_fragment` reject text over a configured byte limit, typed, without
   tokenizing it.
5. All shape arithmetic is checked (`checked_mul`); overflow is a validation error, not a
   wrap or a panic.

**Acceptance criteria**

- [x] A tensor with `shape: [2, 3]` and 5 elements returns `InvalidInput` naming the tensor,
      and no allocation proportional to the declared shape occurs — validation is pure
      arithmetic over `shape` and an element *count*, run before any tensor is built.
- [x] A shape whose product overflows `usize` returns `InvalidInput`, not a panic.
- [x] Passing a tensor name absent from `expected_inputs` returns a contract error citing
      both sets.
- [x] Every `#[allow(clippy::cast_*)]` on the request path is removed. All three became
      checked conversions (`i64::try_from` on input dims, `usize::try_from` on output dims,
      `u32::try_from` on tokenizer offsets), each returning a typed error. The limits that
      would make the old comments true are operator-set, so the conversions prove
      themselves rather than trusting configuration.

### Phase 2: Deadlines and Concurrency Limits

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-phase-deadlines`

**Requirements**

1. Per-model `inference_timeout` (config, with a documented default) bounds a single
   inference; expiry returns `OnnxRunnerError::Timeout { model_id, elapsed_ms }`.
2. A per-model in-flight ceiling — a semaphore sized from replica count — bounds concurrent
   inferences; exhaustion either queues with a bounded wait or returns
   `Overloaded { model_id }`. Which of the two is configurable; the default is recorded in
   the ADR.
3. Queue wait and execution time are measured separately, because they have different causes
   and different fixes.
4. Timeout semantics are documented honestly: ONNX Runtime does not cancel an inference
   mid-graph, so a timeout abandons the *result* while the blocking task runs to completion.
   The consequence — a timed-out call still occupies a replica until it finishes — must be
   stated in the SDK docs, not hidden.
5. The optional caller-supplied deadline on `run` is capped by the per-model config, never
   extended by it.

**Acceptance criteria**

- [x] A model configured with a 1 ms timeout returns `Timeout` for a real inference, and the
      replica becomes usable again afterwards without a rebuild.
- [x] Exceeding the in-flight ceiling produces `Overloaded` rather than unbounded growth;
      asserted with a ceiling of 1 under 8 concurrent calls.
- [x] `Timeout`, `Overloaded`, `ModelUnavailable`, and `InferenceFailed` are four distinct,
      documented outcomes, with the coverage mapping written down once in §5 below.
- [x] The "timeout abandons but does not cancel" caveat appears in the `run` doc comment,
      on `OnnxRunnerError::Timeout`, and on the `inference_timeout` config field.
- [ ] Queue wait and execution time measured **separately** (requirement 3). Not done: the
      deadline measures total elapsed. Separating them needs the histograms in
      [observability](./observability.md), which owns metrics for this gear.

### Phase 3: Output Dtype Strictness

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-phase-dtype-strictness`

**Requirements**

1. Config validation at load requires an `output_dtypes` entry for every name in
   `expected_outputs`; a missing entry fails startup naming the model and the output.
2. The runtime default is removed: an output with no configured dtype is unreachable, and the
   code path returns a typed error rather than assuming F32.
3. A declared dtype that does not match the session's actual output type fails at **load**,
   not at first inference — the session's own output metadata is available and is checked
   alongside the existing name verification.
4. Extraction failures name the tensor, the declared dtype, and the actual dtype.

**Acceptance criteria**

- [x] A config listing an output in `expected_outputs` but not in `output_dtypes` fails
      startup with a message naming both — checked before any I/O, so an unusable config
      does not cost a 400 MB read first.
- [x] A model declaring `f32` for an output the graph emits as `i64` fails at load, with the
      mismatch in the message. The session's own output metadata (`Outlet::dtype`) is
      compared against the declaration alongside the existing name verification.
- [x] No `unwrap_or(OutputDtype::F32)` — or any other silently-assumed dtype — remains. An
      output reaching extraction without a declared dtype is now a typed internal error,
      reachable only if the two load-time checks were bypassed.

## 5. Feature Acceptance Criteria

- [x] No client call can cause an unbounded allocation, an unbounded wait, or a panic.
- [x] Wrong-shaped, wrong-named, and oversized inputs are rejected typed, before work starts.
- [x] No tensor is ever reinterpreted as a dtype it was not declared to be.
- [x] Timeout, overload, unavailable, and failure are four distinguishable outcomes with a
      documented mapping to consumer coverage states — the table below.
- [x] ADR written: bounds, defaults, and the honest limits of timeout semantics —
      [ADR 0006](../../ADR/0006-cpt-cf-onnx-runner-adr-bounds-and-defaults.md).

### Outcome-to-coverage mapping

The single place this correspondence is written down. `onnx-scanner` consumes it; nothing
should re-derive it.

| `OnnxRunnerError` | Retryable | What happened | Consumer coverage |
|---|---|---|---|
| `InvalidInput` | no | The caller built a bad request | Caller bug — not a coverage state; fix the consumer |
| `ContractMismatch` | no | Supplied tensors do not match the model's contract | Caller bug — as above |
| `UnknownModel` | no | No such model is configured | Configuration error — as above |
| `ModelUnavailable` | no | Configured but not loadable, or every replica lost | `Failed` |
| `InferenceFailed` | no | The graph ran and errored | `Failed` |
| `Timeout` | **yes** | Deadline expired; work abandoned, still running | `Failed` |
| `Overloaded` | **yes** | At capacity, load shed | `Failed` |
| `TokenizationFailed` | no | Tokenizer rejected the text | `Failed` |

`OnnxRunnerError::is_retryable` encodes the middle column, so a consumer branches on one
method rather than matching every variant. The three caller-bug rows are deliberately *not*
coverage states: a consumer that reports its own malformed request as degraded coverage is
hiding a bug behind an outage.

## 6. Non-Functional Considerations

**Performance.** Validation is O(rank) arithmetic against inferences costing milliseconds to
seconds; the cost is noise. The semaphore adds one atomic on the happy path.

**Security.** This feature is the gear's DoS surface. Defaults must be safe for an untrusted
caller even though today's caller is trusted, because defaults are what people actually
deploy and because the content being tokenized is attacker-controlled by construction. Error messages carry shapes, names, and
sizes — never tensor contents or tokenized text.

**Compatibility.** Phase 3 is a **breaking config change**: existing YAML that omits an
`output_dtypes` entry will stop starting. Exactly one config in the repo is affected
(`config/genai-security.yaml`, which declares both outputs). Fix it in the same change and
note the break in the ADR and the gear README.

**Consumer impact.** New error variants are additive, but `onnx-scanner` must handle
`Timeout` and `Overloaded` distinctly from `InferenceFailed` to make the coverage mapping
real. Track that as part of the cross-gear criterion above.

## 7. Outcome

**The defaults**, all per-model and overridable: 16Mi elements per tensor (64 MiB as f32),
32 tensors per call, 1Mi extent per dimension, 1 MiB of text, a 30-second inference
deadline, and an in-flight ceiling equal to the replica count. They are set to be safe for
an untrusted caller while being far enough above today's consumer that it never sees them —
the reasoning is in [ADR 0006](../../ADR/0006-cpt-cf-onnx-runner-adr-bounds-and-defaults.md).

**Queue-versus-reject: queue, bounded by the same deadline.** `on_overload` defaults to
`Queue`, so a caller waits for a permit for at most `inference_timeout` and then gets
`Overloaded`. `Reject` is available for deployments that would rather shed immediately. The
argument for queueing as the default is that a consumer's own deadline is usually a better
place to give up than an arbitrary queue depth here, and the wait is already bounded — an
unbounded queue was never one of the options.

**The in-flight ceiling defaults to the replica count** because more inferences in flight
than there are replicas is queueing, not parallelism. Its real purpose is the blocking pool:
[ADR 0004](../../ADR/0004-cpt-cf-onnx-runner-adr-blocking-pool-offload.md) moved unbounded work
onto a *process-wide* pool that other gears share, and explicitly recorded that the cap was
the missing half. This closes it.

**Validation overhead was not measured**, and the claim in §6 that it is noise rests on
argument rather than a number: it is O(rank) integer arithmetic plus one semaphore acquire
against inferences measured in milliseconds to seconds. The benchmark that could confirm it
belongs to [multi-model-execution](./multi-model-execution.md).

**One thing the tests taught.** Writing valid synthetic inputs for `gliner2-pii` took
several attempts against the runtime's rank complaints — `text_word_indices` wants rank 1
where the obvious guess is rank 2. That is direct evidence for the warmup deferral recorded
in [model-lifecycle](./model-lifecycle.md) §7: synthetic inputs generated from a contract
that declares only tensor *names* would have failed exactly this way, and warmup failure was
specified to mark a model unavailable.
