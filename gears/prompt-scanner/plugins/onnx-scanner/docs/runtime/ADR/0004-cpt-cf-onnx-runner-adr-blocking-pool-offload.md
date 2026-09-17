---
status: accepted
date: 2026-08-27
decision-makers: onnx-runner gear maintainers
---

# Run inference on the blocking pool, one session per replica behind an async mutex

**ID**: `cpt-cf-onnx-runner-adr-blocking-pool-offload`

<!-- toc -->

- [Context and Problem Statement](#context-and-problem-statement)
- [Decision Drivers](#decision-drivers)
- [Considered Options](#considered-options)
- [Decision Outcome](#decision-outcome)
  - [Consequences](#consequences)
  - [Confirmation](#confirmation)
- [Pros and Cons of the Options](#pros-and-cons-of-the-options)
  - [spawn_blocking with a per-replica async mutex over an Option slot](#spawn_blocking-with-a-per-replica-async-mutex-over-an-option-slot)
  - [spawn_blocking, keeping the blocking mutex](#spawn_blocking-keeping-the-blocking-mutex)
  - [One session, tuned intra-op threads](#one-session-tuned-intra-op-threads)
  - [An owner thread per replica, driven by a channel](#an-owner-thread-per-replica-driven-by-a-channel)
  - [block_in_place](#block_in_place)
- [More Information](#more-information)
- [Traceability](#traceability)

<!-- /toc -->

## Context and Problem Statement

`run` and `encode` were `async fn`s that did CPU-bound work inline on the calling Tokio
worker, holding a `std::sync::Mutex` while they did it. This gear shares a process with
every other gear, so the cost was never confined to it: four concurrent inferences on a
four-worker runtime stall the runtime completely, and every unrelated task in the process
stops with it.

Two further defects rode along with the same code. A panic inside an inference poisoned
that replica's mutex permanently — every later call to it returned
`Internal("session mutex poisoned")` for the process's lifetime — and the session's ONNX
state after an unwind was unknown, with no rebuild path. And nothing defined what happened
when a caller dropped a `run` future mid-inference.

Nothing had ever exercised the gear concurrently, which is why all three survived as long
as they did.

## Decision Drivers

* Blocking an async worker is the only defect in this gear that damages *other* gears. It
  outranks everything else in the release bar for that reason alone.
* `ort`'s `Session::run` takes `&mut self` because ONNX Runtime's `Run` is not thread-safe.
  Whatever we choose must give each inference exclusive access to a session; sharing one is
  undefined behaviour, not a tuning trade-off (see [DESIGN.md](../DESIGN.md) §2.2 for the
  maintainer's own words).
* Inference here is measured in milliseconds to seconds and dominates the request, unlike
  `file-parser`'s ~3.5 ms detection, which sits inside far costlier document extraction.
  That difference decides which concurrency axis is right.
* A panicking inference must cost at most one session. A gear that permanently disables a
  replica on one bad input is not deployable.
* Cancellation must be safe: a consumer under its own deadline will drop futures, and that
  must not strand or corrupt a session.
* The pattern is already solved in this repo. `file-parser`'s `infra/magika_detector.rs`
  wraps an ONNX session behind an async trait with the cancel-safety argument written out.
  Reinventing it would mean re-deriving a subtle correctness argument.

## Considered Options

* `spawn_blocking` with a per-replica `tokio::sync::Mutex<Option<Session>>`
* `spawn_blocking`, keeping the `std::sync::Mutex`
* One session, tuned intra-op threads (`file-parser`'s choice)
* An owner thread per replica, driven by a channel
* `block_in_place`

## Decision Outcome

Chosen option: **`spawn_blocking` with a per-replica `tokio::sync::Mutex<Option<Session>>`**,
because it is the only option that solves all three defects at once — offload, panic
tolerance, and cancel-safety — and because it is the pattern this repo has already argued
through once.

Callers queue fairly on the async side. `lock_owned()` makes the guard `'static` so it
moves into the blocking task, and the blocking task `take()`s the session out of its slot
for the duration of the call, putting it back on completion.

The `Option` is what buys panic tolerance, and the guard living inside the blocking task is
what buys cancel-safety. Neither is incidental:

* The session is restored on success **and on a returned error** — a rejected input does not
  damage a session. Only an unwind skips the restore, which is exactly when the session must
  not be reused. The slot is left empty and the next caller rebuilds from the same verified
  spec at the same threading settings.
* Because the blocking task owns the guard, it is released by `Drop` when that task ends,
  whether or not anyone still awaits the result. A dropped future therefore cannot strand a
  replica; the in-flight inference simply runs to completion and its result is discarded.

Tokenization moved to the blocking pool too. It is lighter than inference but not free —
large inputs and `onig` regex work — and it had the same shape. It needs no exclusion,
because `Tokenizer::encode` takes `&self`, so an `Arc` clone moves into the blocking task
and any number of encodes proceed concurrently.

Zero replicas became unrepresentable in the same pass: the replica count is a
`NonZeroUsize` fixed at construction, so `ReplicaSelector::next_index` lost the `assert!`
that was a panic in the request path guarded only by a coincidence in another module.

### Consequences

* `LoadedModel::run` is now `async` and takes owned inputs, since they move into the
  blocking task. The gear's client methods await it.
* Each replica is an `Arc<tokio::sync::Mutex<Option<Session>>>`. The `Arc` exists solely so
  `lock_owned` can produce a `'static` guard.
* `spawn_blocking` adds a task hop — single-digit microseconds against inferences measured
  in milliseconds to seconds. Irrelevant here, and worth stating so nobody re-litigates it.
* **The blocking pool is process-wide.** Offloading unbounded work to a shared pool trades
  one starvation problem for another: enough concurrent inferences will occupy every
  blocking thread, and other gears' blocking work queues behind them. An in-flight cap is
  the missing half and belongs to input validation and resource bounds. Until it
  lands, this gear is polite to the async runtime and impolite to the blocking pool.
* A `JoinError` from a panicking blocking task maps to a typed error, never a panic in the
  caller.
* A model configured with `replicas: 0` is rejected at load, before any I/O, with an error
  naming the model and the field, rather than being silently coerced by `.max(1)`.
* Repeated rebuild failure is a real state, not just an error return — see
  [ADR 0005](./0005-cpt-cf-onnx-runner-adr-degraded-startup.md), which took the state
  machine this decision implies and made it observable.
* **The thread budget is still wrong, and this decision does not fix it.**
  `replicas × intra_threads` is never computed, and the default of 4 replicas
  oversubscribes a 10-core host. `with_intra_threads` may also be inert on the OpenMP
  builds Microsoft ships. Both belong to
  execution configuration; this ADR only ensures the
  pool is no longer *also* blocking the async runtime.

### Confirmation

Four tests against a real model, each of which the previous code would fail:

* N concurrent `run` calls with N replicas on a two-worker runtime complete rather than
  deadlocking the runtime.
* An unrelated async task makes measurable progress while an inference is in flight.
* An injected panic returns a typed error, and the *next* call to the same model succeeds
  against a rebuilt session.
* Twenty mid-flight cancellations leave the pool intact. The cancellation must happen
  mid-flight to prove anything: an `async fn`'s future does nothing until first polled, so
  dropping one before awaiting exercises no code at all. Each iteration therefore cancels a
  `timeout` against a sleeping closure.

Plus a source-level guard: a test asserts no blocking mutex appears on an inference path,
because the failure it causes shows up as unrelated latency in other gears, far from the
line responsible.

**Not confirmed: any performance claim.** There is no benchmark. The before/after numbers
this ADR would ideally carry need the harness in
execution configuration, and until that exists the
justification here rests on the defect being structural rather than on measurement.

## Pros and Cons of the Options

### spawn_blocking with a per-replica async mutex over an Option slot

Chosen. Fair async queueing, `lock_owned` into `spawn_blocking`, session taken from its slot
and restored on completion.

* Good, because it solves offload, panic tolerance, and cancel-safety with one structure
  rather than three mechanisms
* Good, because it is `file-parser`'s pattern, already reviewed in this repo, with its
  cancel-safety argument written down
* Good, because waiting happens on the async side, so a queued caller costs a task, not a
  parked OS thread
* Good, because panic containment is structural: the restore is skipped precisely when an
  unwind happens, with no explicit unwind handling to get wrong
* Neutral, because it needs `Arc` on every replica purely to satisfy `'static`
* Bad, because it puts unbounded work on a process-wide pool until an in-flight cap lands
* Bad, because a cancelled request still consumes a full inference's CPU — the work cannot
  be stopped, only its result discarded

### spawn_blocking, keeping the blocking mutex

Wrap the existing body in `spawn_blocking` and leave `std::sync::Mutex` in place.

* Good, because it is the smallest possible change and does get the CPU work off the worker
* Neutral, because a blocking lock inside a blocking task is not itself wrong
* Bad, because poisoning survives: one panic still disables a replica for the process's
  lifetime, which was half the reason to touch this code
* Bad, because contention moves onto blocking-pool threads, so queued callers park OS
  threads instead of tasks — the pool's size becomes the concurrency limit, invisibly
* Bad, because a `MutexGuard` is not `'static`, so the session would have to be moved rather
  than borrowed, which lands back at the `Option` slot anyway — the extra work buys nothing

### One session, tuned intra-op threads

`file-parser`'s choice: a single session, concurrency inside one inference.

* Good, because it is the cheapest in memory — one weight copy, one thread pool
* Good, because it removes replica-count tuning entirely
* Neutral, because it is demonstrably right for `file-parser`, whose detection is small
  against the surrounding extraction
* Bad, because it serializes concurrent requests onto one lock, and inference dominates the
  request here — the opposite of `file-parser`'s situation
* Bad, because intra-op parallelism does not substitute for request concurrency: it shortens
  one inference, it does not let two proceed
* Bad, because `with_intra_threads` may be inert on OpenMP builds, so the one knob this
  option depends on is the one we are least sure works

### An owner thread per replica, driven by a channel

Each replica gets a dedicated OS thread owning its session; callers submit work over a
channel and await a `oneshot`.

* Good, because session ownership becomes unambiguous — no mutex at all
* Good, because it gives a natural place to bound the queue, which the chosen option lacks
* Neutral, because cancel-safety is comparable: a dropped `oneshot` receiver discards the
  result the same way
* Bad, because it is strictly more machinery — N threads, N channels, shutdown handling —
  for the same guarantees
* Bad, because those threads are resident whether or not the model is used, which matters
  when a model may be unavailable
* Bad, because it diverges from the in-repo pattern for no benefit this gear can currently
  name; the queue bound it offers is better served by an explicit in-flight cap that also
  covers tokenization

### block_in_place

Mark the blocking region so Tokio migrates other tasks off the worker.

* Good, because it is a one-line change with no restructuring
* Bad, because it panics on a current-thread runtime, making the gear's correctness depend
  on how the host application configured its runtime
* Bad, because it does not offload — it relocates other work, and the worker is still
  consumed for the inference's duration
* Bad, because it addresses neither poisoning nor cancel-safety

## More Information

The gear had 136 unit tests and no concurrency test, which is exactly why a defect this
structural survived: every existing test called `run` once, from one task, and passed. The
first test written for this change was the one that fails against the old code — N
concurrent calls on a two-worker runtime — so the fix has a witness rather than an
assertion that it seems better.

Which of `file-parser`'s choices were deliberately **not** copied is worth recording, since
the two gears now look similar enough that the differences read as oversights:

* `MagikaDetector` returns `Option` and degrades to "no detection", which suits a detector
  with a fallback. This gear returns `Result` and propagates typed errors, because a
  consumer must be able to tell a failed scan from a clean one.
* `file-parser` chose one session over a pool. Here the pool is mandatory, not preferred.

## Traceability

- **DESIGN**: [DESIGN.md](../DESIGN.md) §2.2, §3.1, §5 items 1-3

This decision directly addresses the following requirements or design elements:

* `cpt-cf-onnx-runner-principle-no-blocking-on-async` — this ADR is that principle's
  implementation and rationale
* `cpt-cf-onnx-runner-phase-blocking-offload` — the offload itself
* `cpt-cf-onnx-runner-phase-panic-recovery` — the `Option` slot and rebuild path
* `cpt-cf-onnx-runner-phase-cancel-safety` — the guard owned by the blocking task
* `cpt-cf-onnx-runner-phase-no-panics` — `NonZeroUsize` replica count, validated config
* `cpt-cf-onnx-runner-component-ort-session` — the component this decision restructures
* `cpt-cf-onnx-runner-principle-bounded-work` — **not** satisfied here; the in-flight cap
  this decision makes necessary is input validation and resource bounds
