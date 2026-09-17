Created:  2026-08-26 by Constructor Tech
Updated:  2026-08-27 by Constructor Tech

# Feature: Execution Safety

<!-- toc -->

- [1. Feature Context](#1-feature-context)
- [2. Current State](#2-current-state)
- [3. Scope](#3-scope)
- [4. Phases](#4-phases)
- [5. Feature Acceptance Criteria](#5-feature-acceptance-criteria)
- [6. Non-Functional Considerations](#6-non-functional-considerations)
- [7. Outcome](#7-outcome)

<!-- /toc -->

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-featstatus-execution-safety`

## 1. Feature Context

- [x] `p1` - `cpt-cf-onnx-runner-feature-execution-safety`

### Overview

Make inference safe to run inside an async service: off the runtime's worker threads,
cancel-safe, panic-tolerant, and free of panics on the request path.

**Realizes**: `cpt-cf-onnx-runner-principle-no-blocking-on-async`.
**Closes**: [DESIGN.md](../../DESIGN.md) §5 items 1-3.

### Purpose

This is the difference between "works when called once" and "can be deployed next to other
gears." A single blocking call on a Tokio worker degrades every task in the process; four
concurrent inferences on a four-worker runtime stall it completely. Nothing had ever
exercised the gear concurrently, which is why the defect survived as long as it did.

The pattern is already established in this repo. `file-parser`'s
`infra/magika_detector.rs` wraps an ONNX session behind an async trait with: a
`tokio::sync::Mutex` for fair async queueing, `lock_owned` so the guard can move into
`spawn_blocking`, the session taken out of its slot so a panic leaves it empty rather than
corrupt, a rebuild path for the next caller, and a documented cancel-safety argument. Copy
its structure; do not reinvent it.

### References

- **Gear design**: [DESIGN.md](../../DESIGN.md) §2.2, §3.1, §5
- **In-repo reference implementation**: `gears/file-parser/file-parser/src/infra/magika_detector.rs`

## 2. Current State

**Implemented.** What follows describes the code as it stood before this feature landed;
it is kept because the acceptance criteria below are written against it.

Previously, `infra/ort_session.rs::run` and `infra/tokenizer.rs::encode`:

- were called from `async fn`s in `gear.rs` and executed inline on the calling worker
  thread;
- took `std::sync::Mutex::lock()` — a blocking lock inside async code;
- poisoned that mutex permanently if an inference panicked, so every subsequent call to
  that replica returned `Internal("session mutex poisoned")` for the process's lifetime;
- left a session's ONNX state unknown after an unwind, with no rebuild path.

`domain/model_registry.rs::next_index` asserted `replica_count > 0`. `LoadedModel::load`
used `cfg.replicas.max(1)`, so the assert was unreachable — but it was a panic in the
request path guarded only by a coincidence in a different module.

Tokenization is lighter than inference but not free (large inputs, `onig` regex work), and
it had the same blocking-on-worker shape.

## 3. Scope

**In scope:**

- Blocking-pool offload for inference and tokenization.
- Async fair queueing per replica.
- Panic tolerance: an inference that panics loses its session, not the gear.
- Cancel-safety: a dropped future never strands or corrupts a session.
- Making zero replicas unrepresentable rather than asserted.

**Out of scope:**

- Deadlines and in-flight caps — [input-bounds](./input-bounds.md).
- Replica-count tuning and provider selection — [multi-model-execution](./multi-model-execution.md).
- Metrics for queue wait — [observability](./observability.md), which depends on the
  queueing primitive this feature introduces.

## 4. Phases

### Phase 1: Offload to the Blocking Pool

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-phase-blocking-offload`

**Requirements**

1. `run` and `encode`/`encode_fragment` execute their CPU-bound work inside
   `tokio::task::spawn_blocking`.
2. Replica selection and locking use `tokio::sync::Mutex` with `lock_owned()`, so the guard
   moves into the blocking task and is released by `Drop` when that task ends.
3. No `std::sync::Mutex` remains on any request path.
4. `LoadedModel` and `LoadedTokenizer` are held in `Arc` so guards satisfy `'static`.

**Acceptance criteria**

- [x] No `std::sync::Mutex` in `infra/`; verified by a `grep`-style test or a clippy lint
      allowance removed.
- [x] A test runs N concurrent `run` calls against a model with N replicas on a
      2-worker-thread runtime and completes; the same test against the old code path would
      stall (record the before/after in the ADR).
- [x] A test proves an unrelated async task makes progress while an inference is in flight.

### Phase 2: Panic Tolerance and Session Recovery

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-phase-panic-recovery`

**Requirements**

1. Each replica slot holds `Option<Session>`. The blocking task `take()`s the session, runs,
   and puts it back on success.
2. A panic inside inference leaves the slot `None`; no poisoning, no corrupt session reuse.
3. The next caller to that slot rebuilds the session from the same verified config, at the
   same threading settings.
4. A rebuild failure is reported as a typed error and the model's health degrades
   ([model-lifecycle](./model-lifecycle.md)) rather than looping.
5. `JoinError` from a panicking blocking task maps to a typed error, never a panic in the
   caller.

**Acceptance criteria**

- [x] A test injects a panic in the inference closure and asserts: the call returns a typed
      error, the slot is empty, and the *next* call to the same model succeeds.
- [x] No `unwrap`/`expect` on lock or join results anywhere in `infra/`.
- [ ] Rebuild reuses the original `intra_threads` and provider settings — implemented via a
      shared `SessionSpec`, but **not yet asserted in a test**.

### Phase 3: Cancel-Safety

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-phase-cancel-safety`

**Requirements**

1. Dropping a `run` future does not stop the in-flight inference; its result is discarded
   and the session returns to its slot.
2. A dropped future never leaves a slot empty-but-not-rebuildable or a guard leaked.
3. The cancel-safety argument is documented at the call site, not only in this file.

**Acceptance criteria**

- [x] A test starts an inference, cancels it before completion, then asserts a
      subsequent call on the same model succeeds.
- [x] A test cancels futures mid-flight in a loop and asserts replica availability is
      unchanged afterwards (no leaked guards). Twenty iterations, each cancelled by a
      `timeout` against a sleeping closure — dropping an *unpolled* future would run no
      code and prove nothing.

### Phase 4: No Panics on the Request Path

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-phase-no-panics`

**Requirements**

1. `ReplicaSelector` cannot be constructed for zero replicas — replica count becomes a
   `NonZeroUsize` established at load time, and `next_index` loses its assert.
2. A model configured with `replicas: 0` fails config validation at load with a clear
   message, rather than being silently coerced by `.max(1)`.
3. Audit of `infra/` and `domain/` for `unwrap`, `expect`, `assert`, slice indexing, and
   integer casts that can wrap; each either removed or given a comment proving it cannot
   fail.

**Acceptance criteria**

- [x] `replicas: 0` in YAML produces a startup error naming the model and the field.
- [x] `next_index` has no `assert!`; the type makes zero unrepresentable.
- [x] No panic-capable call remains unjustified in `infra/` or `domain/`; the shape/cast
      sites carry proofs rather than bare `#[allow]`. Replica indexing carries a proof
      comment; `unwrap`/`expect` are denied workspace-wide by clippy.

## 5. Feature Acceptance Criteria

- [x] Inference and tokenization never occupy an async worker thread.
- [x] A panicking inference costs one session, recovered transparently on the next call.
- [x] A cancelled request never damages a session or leaks a permit.
- [x] No panic is reachable from a client call, for any configuration accepted at startup.
- [x] Concurrency behavior is covered by tests that would fail against the current code.
- [x] ADR written: blocking-pool offload and cancel-safety model (backlog item 5) —
      [ADR 0004](../../ADR/0004-cpt-cf-onnx-runner-adr-blocking-pool-offload.md).

## 6. Non-Functional Considerations

**Performance.** `spawn_blocking` adds a task hop (single-digit microseconds) against
inferences measured in milliseconds to seconds — irrelevant. The blocking pool is shared
process-wide, so an in-flight cap matters ([input-bounds](./input-bounds.md)); until that
lands, note the interaction in the ADR.

**Threading interaction.** ONNX Runtime's intra-op pool parallelizes *within* one inference
while replicas parallelize *across* inferences. Both consume cores. Reference:
`file-parser` deliberately chose one session and tuned intra-op threads instead, because its
detection cost was small relative to the surrounding request. Our inference dominates, so
replicas are the right axis — but the total thread budget (`replicas × intra_threads`) needs
stating in the ADR, since 4 × default on a 10-core host oversubscribes badly.

**Security.** No payload content in any error, log, or span added here.

## 7. Outcome

The concurrency model: one `Arc<tokio::sync::Mutex<Option<Session>>>` per replica, fair
FIFO queueing on the async side, `lock_owned` so the guard moves into `spawn_blocking`,
and the session taken out of its slot for the duration of the call. It is restored on
success and on a returned error alike; only an unwind skips the restore, which is exactly
when the session must not be reused. Cancellation is safe because the blocking task owns
the guard and drops it on completion, whether or not anyone still awaits the result.

Copied from `file-parser`'s `magika_detector.rs`: the `Option` slot, `lock_owned`, the
rebuild path, and the bounded abandonable-thread pattern for startup.

Deliberately **not** copied:

- **Swallowing failures.** `MagikaDetector` returns `Option` and degrades to "no
  detection", which suits a detector that has a fallback. This gear returns `Result` and
  propagates typed errors — a consumer must be able to tell a failed scan from a clean one.
- **One session instead of a pool.** `file-parser` chose a single session because its
  detection (~3.5 ms) is small against the surrounding document extraction. Here inference
  dominates, so replicas are the right axis. The pool is also not optional: `ort` takes
  `&mut self` on `Session::run` because ONNX Runtime's `Run` is not thread-safe.

Still open: no benchmark. The before/after this section was meant to record needs the
harness from [multi-model-execution](./multi-model-execution.md), and the total thread
budget (`replicas × intra_threads`) remains unstated — 4 × default on a 10-core host
oversubscribes. Both belong there, and to the ADR still owed.
