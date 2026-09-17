Created:  2026-08-26 by Constructor Tech
Updated:  2026-09-04 by Constructor Tech

# onnx-runner — Spike to Release Plan

<!-- toc -->

- [1. Current Position](#1-current-position)
- [2. Release Definition](#2-release-definition)
- [2a. Iteration 1: what actually ships](#2a-iteration-1-what-actually-ships)
- [3. The Chain](#3-the-chain)
  - [Stage 0 — Correct the Record](#stage-0--correct-the-record)
  - [Stage 1 — Execution Safety](#stage-1--execution-safety)
  - [Stage 2 — Availability](#stage-2--availability)
  - [Stage 3 — Bounds](#stage-3--bounds)
  - [Stage R — ONNX Runtime Provisioning *(unplanned)*](#stage-r--onnx-runtime-provisioning-unplanned)
  - [Stage 4 — Observability](#stage-4--observability)
  - [Stage 5 — Configuration and Measurement](#stage-5--configuration-and-measurement)
  - [Stage 6 — Contract and Health](#stage-6--contract-and-health)
  - [Stage 7 — Close Out](#stage-7--close-out)
- [4. Dependency Graph](#4-dependency-graph)
- [5. If You Get Blocked](#5-if-you-get-blocked)
- [5a. What landed outside this plan](#5a-what-landed-outside-this-plan)
- [6. Estimates](#6-estimates)
- [7. Interaction With the Program Plan](#7-interaction-with-the-program-plan)
- [8. Triaged out of iteration 1](#8-triaged-out-of-iteration-1)

<!-- /toc -->

## 1. Current Position

> **Next up:** the iteration-1 close-out list in [§2a](#2a-iteration-1-what-actually-ships).
>
> **In progress:** nothing.
>
> **Blocked:** nothing.
>
> **Done:** Stages 0, 1, 3, 4, R, 7.1-7.5, Stage 2 apart from warmup, and Stage 5's
> measurement half — **5.0 and 5.4**, which landed together with a body of unplanned
> performance and model-selection research (see [§5a](#5a-what-landed-outside-this-plan)).
>
> **Remaining for iteration 1:** the cheap half of 5.3, 5.5, 7.6-7.9. Stage 6 is
> **complete** — 6.1, 6.2, 6.3 and 6.4 all landed — and so is 5.1. Everything else is
> triaged into [§8](#8-triaged-out-of-iteration-1).
>
> Update these lines whenever you stop working.

## 2. Release Definition

"Released" for this gear means: another team can depend on it for a use case we did not
write, deploy it next to other gears without degrading them, operate it without reading the
source, and extend it without asking us. Concretely, all of:

- [x] No blocking work on async workers; no panic reachable from a client call.
- [x] One bad model artifact cannot stop the process.
- [x] Every call bounded in size, time, and concurrency.
- [x] No tensor ever reinterpreted as a dtype it was not declared to be.
- [x] `infra/` under test; a real session exercised in CI; tokenizer offsets proven.
      *(A committed 285-byte fixture puts a real session in CI. The model-backed concurrency
      suites remain feature-gated, but CI compiles them.)*
- [x] Inference time and queue wait observable per model.
- [x] The replica pool is measured, not believed, and the shipped defaults come from the
      measurement.
- [ ] `execution_provider` real, so the documentation is true.
      **Reinterpreted for iteration 1:** the documentation is made true by *recording that
      this gear is CPU-only and why* (ADR 0009), not by adding a field whose non-CPU paths
      we cannot test. See [§8](#8-triaged-out-of-iteration-1).
- [x] Model readiness visible through platform `/health` and `/readyz`; SDK types serializable.
- [ ] Ten ADRs (9 written — 0009, 0010 and the `ort`-pin ADR remain), a PRD, a real README,
      a `GEARS.md` entry.

Deliberately *not* in the release bar: batching, runtime model reload, GPU certification,
gRPC/OoP transport, and any HTTP surface of the gear's own
(`cpt-cf-onnx-runner-constraint-no-http-surface` — a standing non-goal, not deferred work). Those are [DECOMPOSITION.md](./DECOMPOSITION.md) §2 NICE/LATER items and
each is additive.

## 2a. Iteration 1: what actually ships

The chain below was written as one release. It is now split: **iteration 1 is the smallest
thing that is safe to put in front of another team**, and everything that is real work but
not a safety, honesty or operability requirement is triaged into [§8](#8-triaged-out-of-iteration-1)
as a follow-up task.

The test applied to each remaining item was: *if we ship without this, does a consumer get
hurt, does an operator fly blind, or does a document lie?* Three yeses stay; everything else
goes.

| # | Item | Why it is in iteration 1 | Est |
|---|---|---|---|
| 5.1 | Verify concurrent multi-model hosting | The one unproven claim in the gear's purpose. Two models hosted at once is what `prompt-injection-detection` needs, and it has never been executed. Fixture-based, so it is test work, not design work. | 0.5d |
| 5.3a | Thread budget: computed, logged, warned | An operator who oversubscribes a host today gets no signal at all, and the benchmark showed the shipped default is a quarter of achievable. Cheap, and directly actionable from measurements we already have. | 0.5d |
| 5.3b | Measured defaults in `config/genai-security.yaml` | The benchmark's output is worthless until the config reflects it. `replicas: 1` ships today against a measured optimum of 12x4. | 0.25d |
| 6.1 | Serializable SDK types | serde derives on eight types. Half a day, and it is a public-contract decision that is far cheaper now than after consumers exist. | 0.5d |
| 6.2 | Platform healthcheck | A gear serving zero models currently reports healthy. That is the operability defect, and it is ~20 lines against the toolkit registry. | 0.5d |
| 6.3 | The `SecurityContext` position | A decision plus a doc comment. Shipping a gear whose authorization posture is unstated is how an unstated posture becomes an assumed one. | 0.25d |
| 5.5 | ADRs: replica-pool outcome, CPU default | The pool sizing and the CPU-only posture are now *measured*. An unrecorded measurement gets re-litigated. | 0.5d |
| 6.4 | ADR: `SecurityContext` position | With 6.3. | 0.25d |
| 7.6 | Gear PRD | Repo convention for every mature gear; this gear's purpose has never been written outside a design sketch. | 0.5d |
| 7.7 | README, `api/README.md`, `GEARS.md` entry | The README is two lines and the catalog does not mention the gear. A gear nobody can find is not released. | 0.5d |
| 7.8 | ADR: `ort` pre-release pin and the `load-dynamic` posture | Publishing on `=2.0.0-rc.12` with an operator-provisioned dylib is inherited risk that must be stated, not discovered. | 0.25d |
| 7.9 | Release review | Walk §2. Every box ticked or explicitly waived with a reason. | 0.5d |

**Total: ~5 days.** Against ~8.5 in the pre-split plan.

## 3. The Chain

One serial queue. Each step is a phase from a feature file; the feature file holds the
requirements and acceptance criteria, this file holds *order* and *why here*.

### Stage 0 — Correct the Record

*0.5 days. Do it first because everything downstream reasons from these documents.*

- [x] **0.1 — Rewrite DESIGN and DECOMPOSITION against the actual code**
  → [DESIGN.md](../DESIGN.md), [DECOMPOSITION.md](./DECOMPOSITION.md)

  Done in this pass. Two false claims removed (`execution_provider`, the models endpoint),
  the spike's maturity stated plainly, gaps enumerated and prioritized.

- [x] **0.2 — ADRs for the decisions that need no new work** · 0.5d
  → [DECOMPOSITION.md](./DECOMPOSITION.md) §5 items 1, 1a, 4

  Done. Three ADRs, all settled decisions with no implementation dependency:

  - [0001 — generic tensor layer](../ADR/0001-cpt-cf-onnx-runner-adr-generic-tensor-layer.md)
    (option A over B; the constraint every other decision is measured against)
  - [0002 — out-of-band model provisioning](../ADR/0002-cpt-cf-onnx-runner-adr-out-of-band-model-provisioning.md)
  - [0003 — no HTTP surface](../ADR/0003-cpt-cf-onnx-runner-adr-no-http-surface.md)
    (pulled forward from step 6.4 — the decision was taken, and recording it is what stops the
    removed REST claim from returning)

  They also set the template for the remaining ADRs.

### Stage 1 — Execution Safety

*4 days. Do not reorder. Every stage below edits the same code paths.*

- [x] **1.1 — Blocking-pool offload** · `cpt-cf-onnx-runner-phase-blocking-offload` · 1.5d
- [x] **1.2 — Panic tolerance and session recovery** · `cpt-cf-onnx-runner-phase-panic-recovery` · 1d
- [x] **1.3 — Cancel-safety** · `cpt-cf-onnx-runner-phase-cancel-safety` · 0.5d
- [x] **1.4 — No panics on the request path** · `cpt-cf-onnx-runner-phase-no-panics` · 0.5d
- [x] **1.5 — ADR: concurrency model** · 0.5d → [ADR 0004](../ADR/0004-cpt-cf-onnx-runner-adr-blocking-pool-offload.md)

→ [execution-safety.md](./features/execution-safety.md)

**Why first.** It is the only defect that damages *other* gears in the same process, and it
changes the shape of every call path the later stages touch. Do it after the benchmark and
you benchmark a bug; do it after the REST surface and you rewrite the handlers.

Copy `file-parser`'s `infra/magika_detector.rs` structure — same problem, already solved to
standard in this repo, with the cancel-safety argument written out.

**Note on 1.1:** the first test to write is the one that fails today — N concurrent calls on
a 2-worker runtime. Write it before the fix so the fix has a witness.

**Outcome.** Done as planned; the witness test was written first. `file-parser`'s structure
was copied, with two departures recorded in the ADR: this gear returns `Result` rather than
degrading to `Option`, and its replica pool is mandatory rather than preferred. No benchmark
— the before/after numbers this stage would ideally carry still need Stage 5.

### Stage 2 — Availability

*3 days. Depends on Stage 1 (session rebuild is the same state machine).*

- [x] **2.1 — Per-model state** · `cpt-cf-onnx-runner-phase-model-state` · 1d
- [x] **2.2 — Non-fatal startup** · `cpt-cf-onnx-runner-phase-degraded-startup` · 1d
- [◐] **2.3 — Readiness and warmup** · `cpt-cf-onnx-runner-phase-readiness-warmup` · 0.5d
      — readiness done, **warmup deferred** (see the note below)
- [x] **2.4 — ADR: degraded startup** · 0.5d → [ADR 0005](../ADR/0005-cpt-cf-onnx-runner-adr-degraded-startup.md)

→ [model-lifecycle.md](./features/model-lifecycle.md)

**Cross-gear decision in 2.2.** `require_all_models` defaults to permissive, which means
`onnx-scanner` must map an unavailable model to `Failed` coverage. That mapping is
`prompt-scanner`'s `coverage-reporting` Phase 2. Neither side is done alone — coordinate, or
the permissive default silently produces clean scans, which is the exact defect the program's
largest gap is about.

**Outcome.** 2.1, 2.2 and 2.4 done. The cross-gear warning above held: the producer side is
complete (`ModelUnavailable` with a stable reason, distinct from `UnknownModel`), and the
consumer-side mapping in `coverage-reporting` Phase 2 is **not** written, so that criterion
is deliberately left open rather than claimed.

**2.3 is half done, and the missing half is a decision, not typing.** Readiness shipped:
model state and live replica count are read at call time, and a model that loses every
replica transitions to `Unavailable`. Warmup did not. Its spec requires synthetic inference
from a contract that declares only tensor *names*, and requires warmup failure to mark a
model unavailable — for `gliner2-pii`'s four interdependent inputs, plausible synthetic
values are semantically out of range, so warmup would take a *working* model out of service.
Stage 3's tests later confirmed this by accident: valid inputs took several attempts against
the runtime's rank complaints. Warmup needs per-model warmup inputs in config, or a richer
contract. Reopen it with that decision, not before.

### Stage 3 — Bounds

*3 days. Depends on Stages 1-2.*

- [x] **3.1 — Input validation** · `cpt-cf-onnx-runner-phase-input-validation` · 1d
- [x] **3.2 — Deadlines and concurrency limits** · `cpt-cf-onnx-runner-phase-deadlines` · 1d
- [x] **3.3 — Output dtype strictness** · `cpt-cf-onnx-runner-phase-dtype-strictness` · 0.5d
- [x] **3.4 — ADR: bounds and defaults** · 0.5d → [ADR 0006](../ADR/0006-cpt-cf-onnx-runner-adr-bounds-and-defaults.md)

→ [input-bounds.md](./features/input-bounds.md)

**3.3 is a breaking config change** — YAML omitting an `output_dtypes` entry stops starting.
One config in the repo is affected and already compliant; fix it in the same commit.

**Outcome.** All four done. Both predictions held exactly: the breaking change affected one
already-compliant config, and the timeout honesty requirement is now documented in three
places. Two things the stage picked up beyond its brief — all three `#[allow(clippy::cast_*)]`
on the request path became checked conversions, and the outcome-to-coverage mapping is
written down once in [input-bounds.md](./features/input-bounds.md) §5. One requirement is
**not** met: queue wait and execution time are not measured separately, which needs Stage 4's
histograms.

**3.2 has an honesty requirement.** ONNX Runtime does not cancel mid-graph, so a timeout
abandons the result while the work runs to completion. Document that in the SDK, do not paper
over it.

### Stage R — ONNX Runtime Provisioning *(unplanned)*

*~1.5 days, already spent. Not in the original chain: this plan predates the discovery.*

- [x] **R.1 — Diagnose the startup hang** — `ort` 2.0.0-rc.12 blocks forever instead of
      erroring when the ONNX Runtime it `dlopen`s cannot be loaded. Confirmed by stack
      sampling: 0% CPU, parked in `ort::api()`'s `OnceLock`.
- [x] **R.2 — Remove `ort`'s `download-binaries`/`copy-dylibs`** from this gear. They link
      statically, and Cargo unifies features workspace-wide, so they cannot coexist with the
      workspace's `load-dynamic`. That combination is *why this gear hung where `file-parser`
      did not* — and CI had documented the conflict before anyone hit it.
- [x] **R.3 — `libs/ort-bootstrap`** — bounded, abandonable init shared with `file-parser`:
      raw `std::thread` (not `spawn_blocking`, which Tokio joins on shutdown), `oneshot`,
      timeout, cancellation race, and `leaked_init_thread` as a checkable obligation.
- [x] **R.4 — `scripts/fetch-onnxruntime.sh`** — pinned version + per-platform checksums,
      repo-level because two gears need it. `make onnxruntime-fetch`, wired into
      `genai-security-setup` and the demo runner.
- [x] **R.5 — CI** — `test-magika` became `test-onnxruntime`, uses the shared script, and now
      also runs this gear's model-free tests. The change filter previously fired on
      `file-parser` only, so an `onnx-runner` change triggered no runtime-dependent job at all.

**Why this belongs in the record.** Every stage below assumes a working ONNX Runtime. Without
this, none of them can be tested, and the failure mode is a silent hang rather than an error —
the worst possible first experience for anyone picking the gear up. It also means the
gear's startup is now bounded against a hang it can neither detect nor interrupt.

**Still owed from here:** 7.8's ADR should now cover the `load-dynamic` posture and the
runtime-provisioning story, not just the pre-release pin.

### Stage 4 — Observability

*2 days. Depends on Stages 1-2. Deliberately before the benchmark.*

- [x] **4.1 — Metrics** · `cpt-cf-onnx-runner-phase-metrics` · 1d
- [x] **4.2 — Spans and the payload-free guarantee** · `cpt-cf-onnx-runner-phase-spans` · 0.5d
- [x] **4.3 — ADR: what is measured** · 0.5d → [ADR 0008](../ADR/0008-cpt-cf-onnx-runner-adr-what-is-measured.md)

→ [observability.md](./features/observability.md)

**Why before Stage 5.** The benchmark should read the same instruments operators will read.
Ad-hoc timing code in a benchmark measures something no one can reproduce in production, and
gets thrown away.

**Outcome.** Nine instruments behind a domain-owned port, with queue wait and execution as
*separate* histograms — tested in both directions, since either alone would pass against one
merged number. That separation is what makes 5.4's replica sweep interpretable at all.

The sentinel suite's anti-vacuous guard earned its keep immediately: three cases failed
because calls rejected at the **client dispatch** layer — unknown model, unavailable model,
oversized text — emitted no telemetry whatsoever. Only the session layer had been
instrumented, so the three most common misconfigurations were invisible. Fixed by
instrumenting the client methods.

Its teeth were then verified against the real code: adding `text = %text` to the tokenizer
span made the suite fail with the leaked string quoted, and was reverted.

Not done: instrumentation overhead is argued, not measured, and resident bytes is an
estimate (`replicas × file size`) explicitly labelled a floor. Both belong with 5.4.

The sentinel test in 4.2 — a known string in input must never appear in logs or spans — is
the only mechanical control on
`cpt-cf-onnx-runner-principle-no-payload-in-telemetry`. Treat its later removal as a defect.

### Stage 5 — Configuration and Measurement

*5.5 days. Depends on Stages 1, 2, 4.*

- [x] **5.0 — Establish whether the thread budget is controllable at all** · 0.5d
      — **answered: it is.** See the finding below.
- [x] **5.1 — Verify concurrent multi-model hosting** · `cpt-cf-onnx-runner-phase-multi-model-verify` · 0.5d
      — **done.** A second committed fixture (`other.onnx`, 307 bytes) that disagrees with the
      first on names, arity and dtypes; nine tests. No defect found. The rescope held: it was
      test work, not design work.
- [—] **5.2 — Real execution settings** · `cpt-cf-onnx-runner-phase-exec-settings`
      — **triaged out** ([§8](#8-triaged-out-of-iteration-1)); replaced by ADR 0009 in 5.5
- [◐] **5.3 — Fix the thread and memory model** · `cpt-cf-onnx-runner-phase-thread-model`
      — split: **5.3a/5.3b** (budget log, measured defaults) in iteration 1;
      pre-packed weights and the global pool **triaged out**. Requirement 3 (OpenMP) is
      already discharged by 5.0.
- [x] **5.4 — Concurrency benchmark** · `cpt-cf-onnx-runner-phase-benchmark` · 1d
      — **done.** See the outcome below.
- [ ] **5.5 — ADRs: CPU default, replica-pool outcome** · 0.5d
      — *iteration 1*. The settings-model ADR goes with 5.2, which is triaged out.

→ [multi-model-execution.md](./features/multi-model-execution.md)

**5.3 exists because the replica pool is paid for twice.** Verified against `ort` rc.12/rc.13
sources:

- Each `Session` builds its own intra-op thread pool unless the *environment* has a global
  one, so N replicas means N weight copies **and** N thread pools. `replicas × intra_threads`
  is never computed; the current default oversubscribes a 10-core host.
- `with_intra_threads` has **no effect on OpenMP builds**, which is what `download-binaries`
  fetches from Microsoft. Our `intra_threads: 1` may be inert today.
- `PrepackedWeights` (shared weights across sessions of one model) and
  `with_global_thread_pool` → `DisablePerSessionThreads` (shared pool) are ORT's own answers
  to both costs, and neither is used.
- **What 5.3 must not do:** replace replicas with one shared session. `ort` makes
  `Session::run` take `&mut self` deliberately — ONNX Runtime's `Run` is not thread-safe, and
  the maintainer's comment at `session/mod.rs:296` calls sharing it outright UB, despite
  `Session` being `Send + Sync`. Replicas are the only concurrency mechanism `ort` 2.x offers.
  The job is making them cheap, not removing them. [DESIGN.md](../DESIGN.md) §2.2 has the
  quote.
- The global-pool half is an **application** decision, not the gear's: `ort` documents that a
  library must never configure the environment, and the environment cannot be reconfigured
  after commit. The gear must work correctly either way; the wiring belongs to
  `cf-gears-example-server` / toolkit bootstrap.

**5.0 — FINDING: `intra_threads` works. The benchmark is unblocked.**

The concern was that `ort` documents `with_intra_threads` as inert on OpenMP builds, which
would make the fixed-thread-budget comparison impossible to construct. Measured on a MatMul
chain (dim 512, 8 layers, batch 64), release build, macOS arm64:

| `intra_threads` | per call |
|---|---|
| 1 | 2.78 ms |
| 2 | 1.41 ms |
| 4 | 0.72 ms |
| 8 | 0.58 ms |

Near-linear to 4 threads, tapering at 8 — the shape of real intra-op parallelism, not of an
ignored setting.

**Why the original concern no longer applies:** it was recorded when the gear used `ort`'s
`download-binaries`, which links statically. Stage R removed that feature and switched to the
official dynamic releases. Neither the macOS arm64 nor the linux-x64 1.29.0 binary links
OpenMP — no `libgomp` among the Linux `NEEDED` entries, no OpenMP symbols in either. So the
premise changed underneath the concern, and Stage R is what changed it.

**Caveats worth keeping:** this was measured on one platform with one graph shape. A future
runtime build could reintroduce OpenMP, and `ORT_DYLIB_PATH` means an operator can substitute
any library they like. Phase 3 requirement 3 should therefore still *log* the effective
thread budget rather than assume it, and 5.4 should record which runtime build produced its
numbers.

**Outcome of 5.4 — the pool is vindicated, and the shipped default is wrong.**

The harness is `src/bin/benchmark.rs`: a purpose-built binary (not `criterion`), one
configuration per process invocation, reading [observability](./features/observability.md)'s
instruments rather than timing by hand, sweeping all four axes with shed calls and peak RSS
reported per configuration. Runs are committed under
[docs/benchmarks/](../../benchmarks/), with [PERFORMANCE-RESEARCH.md](../../benchmarks/PERFORMANCE-RESEARCH.md)
carrying the conclusions and the dated files carrying the raw tables. Measured on a laptop
and on a 48-physical-core server (`bg-g3`).

What it settled, against the acceptance criteria as written:

- **One fat session or several thin ones? Several thin ones** — at a fixed 48-thread budget,
  many thin replicas beat one fat session by ~4x under concurrent load. A single session
  cannot use 48 threads across two sockets. The pool design is confirmed, not refuted.
- **The crossover is concurrency, not input size.** The prediction that input size would
  decide the winner was wrong: at one caller, threads always win (a lone caller cannot keep
  several replicas busy); at four or more, replicas always win. Input size moves the margin,
  never the winner. The four-axis sweep is what makes that statement possible — sweeping
  three axes would have produced the input-size story we expected.
- **But the extreme is not the optimum.** 24 replicas buys 23% more throughput than 6 for
  3.6x the memory, and is *slower* at long prompts. Measured defaults are 12x4 for a large
  model, 24x2 for a small one, 4x1 laptop-class, 1 x all-available for a serial caller.
- **The workload is memory-bandwidth-bound.** The best arrangement reaches ~36% of linear
  scaling from single-thread performance, and throughput stops improving past the replica
  count. Hyperthreads do nothing. Adding cores will not help this gear much.
- **Model choice dominates every knob by an order of magnitude.** 12x across the candidate
  models, against ~1.3x for every runtime knob multiplied together. Recorded because it
  redirects effort away from this gear: tuning the runner is not the lever.

**Two acceptance criteria are not met and carry into iteration 1.** The measured default is
not in `config/genai-security.yaml` (still `replicas: 1`, a quarter of achievable) — that is
5.3b. And the replica-pool ADR is unwritten — that is 5.5. A measurement nobody configures
from and nobody can cite is not finished work.

**5.4 may refute the current pool sizing.** Benchmark at a *fixed thread budget* — 4 replicas
× 1 thread against 1 replica × 4 threads — not against 1 × 1, which would flatter the pool by
giving it four times the CPU. If N replicas show no benefit at equal budget, that is a real
finding: record it, set the default from measurement, open a follow-up. Do not redesign
mid-chain.

**5.4 is now unblocked by 5.0.** The 4 replicas × 1 thread against 1 replica × 4 threads
comparison can be constructed as specified.

**Four axes, not three.** Replica count, intra-op threads, concurrency, **and input size**.
Replica benefit is a function of input size — small inputs are dominated by framework
overhead and make the pool look useless, large ones are dominated by compute and may favour
intra-op threading. Sweeping replicas at one input size can produce either verdict, and both
would be wrong. The crossover is the number an operator actually sizes a deployment from.

**Three things landed after this plan was written that will distort 5.4 unless handled.**
All three come from Stage 3, which did not exist when the benchmark was specified:

- `max_in_flight` defaults to the replica count. A concurrency sweep past that ceiling
  measures the semaphore rather than contention. Raise it deliberately, or make it a fifth
  axis.
- `inference_timeout` defaults to 30s. Large inputs at high concurrency can hit it, silently
  dropping completed work and improving the percentiles.
- `Overloaded` and `Timeout` must be counted and reported separately. Throughput over a
  denominator that includes shed calls is not throughput.

**The fixture generator from Stage 7.1 changes what 5.4 can benchmark.** It was built for
tests, but it emits arbitrary graphs — so a ladder of synthetic models with tunable compute
(`MatMul` over sized initializers) is now cheap. That makes compute cost a *controlled*
variable rather than whatever `gliner2-pii` happens to do, and it is kilobytes rather than
1.8 GB at four replicas. Use it for the controlled sweep; keep one real-model run for
validation, because a synthetic graph answers "does the pool work" and only a real one
answers "does that hold for what we ship".

**Expect the teardown race.** A replica sweep loads and drops many sessions, which is the
pattern that intermittently aborts at process exit (see Stage 7.1). Run one configuration per
process invocation.

**5.1 is the de-risking step for the program.** Its external acceptance test is that
`prompt-injection-detection` can add a second model with configuration changes only.

**Outcome.** Done, no defect found — stated carefully, because "nothing was wrong" is the
easiest result to overclaim. Proven: independent contracts, digests, tokenizers and replica
pools per model; dispatch by id cannot return another model's answer; *served* /
*configured-but-broken* / *never configured* stay three distinct outcomes; 24 interleaved
concurrent calls across two models each get their own payload back. Not proven: anything at
real-model scale — both fixtures are a few hundred bytes, and memory and contention at 446 MB
× 4 replicas is Phase 4's question. The producer half of the external test is met (the second
model is added through `ModelConfig` alone, no gear source touched); the consumer half is
`prompt-injection-detection`'s action in another repo and stays open there.

### Stage 6 — Contract and Health

*2 days. Depends on Stages 2, 3 (per-model state is what the healthcheck reports).*

- [x] **6.1 — Serializable SDK types** · `cpt-cf-onnx-runner-phase-sdk-serde` · 0.5d
- [x] **6.2 — Platform healthcheck** · `cpt-cf-onnx-runner-phase-healthcheck` · 0.5d
- [x] **6.3 — The SecurityContext position** · `cpt-cf-onnx-runner-phase-authz-position` · 0.5d
- [x] **6.4 — ADR: SecurityContext position** · 0.25d  *(no-HTTP-surface landed in 0.2)*
      → [ADR 0011](../ADR/0011-cpt-cf-onnx-runner-adr-security-context-position.md)

→ [sdk-contract-and-health.md](./features/sdk-contract-and-health.md)

**No HTTP endpoints here.** Earlier revisions of this gear's docs implied a
`GET /onnx-runner/v1/models` endpoint had shipped; it never existed, and it is now a recorded
non-goal. Operator readiness goes through the platform `Healthcheck` registry
(`libs/toolkit/src/healthcheck.rs`) — API Gateway aggregates it into `/health` and `/readyz`,
which is the whole operator need for ~20 lines. serde on the SDK types stands on its own
legs: it unblocks a future OoP/gRPC transport, which is the plausible way to scale an
inference gear, and it is not a step toward JSON tensors over HTTP.

**6.3 needs a decision, not an implementation, first.** Recommended: this gear is a trusted
in-process compute primitive with no tenant-scoped resources, so `SecurityContext` stays for
trace propagation and its non-enforcement is documented at the trait. If that is rejected,
enforcing authorization is a larger piece of work needing its own estimate — surface it before
starting 6.3, not during.

**Outcome.** All four steps done. The recommendation was put to the maintainer and taken, so no
enforcement work was opened. Two of Phase 3's five requirements were already satisfied
(`is_retryable`, `#![forbid(unsafe_code)]` on both crates), leaving the decision and its
documentation. Two things the stage picked up beyond its brief: `ModelFailureReason` now has a
single `snake_case` spelling shared by the wire, the logs and the healthcheck body, pinned by a
test; and the gear had to declare `capabilities = [rest]` for the healthcheck hook to be called
at all — `register_rest` returns the router untouched, and `registers_no_rest_routes` drives
four paths through it, including the `/onnx-runner/v1/models` endpoint the old docs falsely
claimed.

### Stage 7 — Close Out

*5.5 days. Test-suite work is threaded through everything above; this is what remains.*

- [x] **7.1 — Fixture model** · `cpt-cf-onnx-runner-phase-test-fixture` · 1d

  **Pull this forward — it is now overdue.** It was listed here because the suite completes
  here, but every stage's tests are easier with a loadable few-KB model. Stages 1-3 all
  shipped their model-backed tests behind a `real-model-tests` feature instead, so CI runs
  only the model-free subset and the coverage claim in §2 stays partial.

  **A second reason has since appeared.** Loading and dropping several 446 MB sessions in one
  process occasionally aborts at *exit* — `libc++abi: recursive_mutex lock failed` — after
  every assertion has passed. It is a race in ONNX Runtime's own teardown (the class
  `file-parser` records), seen once in five runs of the lifecycle suite. A few-KB fixture
  makes it rare enough to ignore; the current suite does not.

- [x] **7.2 — `infra/` coverage** · `cpt-cf-onnx-runner-phase-test-infra` · 1.5d
- [x] **7.3 — Tokenizer offset correctness** · `cpt-cf-onnx-runner-phase-test-offsets` · 1d
- [x] **7.4 — CI integration** · `cpt-cf-onnx-runner-phase-test-ci` · 0.5d
- [x] **7.5 — ADR: fixture strategy** · 0.5d → [ADR 0007](../ADR/0007-cpt-cf-onnx-runner-adr-test-fixture-strategy.md)

→ [test-suite.md](./features/test-suite.md)

**7.3 can produce a cross-program finding.** If tokenizer offsets turn out to be character
offsets rather than byte offsets, every redaction span in the scanning pipeline is wrong for
non-ASCII input. Do not adjust the test to pass — escalate.

**Outcome (7.1-7.5).** Done, and out of order: the test suite was pulled forward past Stages
5 and 6 because every stage was shipping its model-backed tests behind a feature flag that CI
never ran. A generated 285-byte ONNX fixture now means the default suite loads a real session
with no download.

**The offset question resolved the reassuring way: they are byte offsets.** `привет` is 6
characters and 12 bytes and returns `(0, 12)`. No escalation needed. Worth noting why it was
worth proving — a character-offset implementation would have returned `при` instead of
`привет`: a shorter, still-valid slice, silently wrong rather than loudly broken.

**Still gated:** the concurrency, panic-recovery and cancel-safety suites need a real model,
so they stay behind `real-model-tests`. CI compiles them so they cannot rot. Porting them to
the fixture is the natural follow-up.

- [ ] **7.6 — Gear PRD** · 0.5d

  What this gear is for, who consumes it, what it promises. Every mature gear here has one;
  this gear has never had its purpose written down outside a design sketch.

- [ ] **7.7 — README, `api/README.md`, `GEARS.md` entry** · 0.5d

  The README is two lines. `GEARS.md` has a "Model Runtime Controller" section with TODO
  links and no mention of this gear — that is where it belongs in the catalog.

- [ ] **7.8 — ADR: `ort` pre-release pin** · 0.25d

  `=2.0.0-rc.12`, forced by `magika`'s hard pin elsewhere in the workspace. Publishing a gear
  on a pre-release dependency, plus onnxruntime binary distribution and licensing, is a
  decision that should be recorded rather than inherited silently.

- [ ] **7.9 — Release review** · 0.5d

  Walk §2's release definition. Every box ticked or explicitly waived with a reason.
  [DECOMPOSITION.md](./DECOMPOSITION.md) §2 has no NEED row left in `✗`.

## 4. Dependency Graph

```
  0.2 ADRs (no deps)
  R.  onnxruntime provisioning ── everything that loads a session needs it
  7.1 fixture model ──▶ done; also unblocks 5.4's synthetic model ladder

  1. execution-safety ──┬──▶ 2. model-lifecycle ──┬──▶ 3. input-bounds
                        │                          ├──▶ 6. public-api
                        │                          └──▶ 4. observability ──▶ 5. exec-config
                        └──▶ (all of the above)

  3. input-bounds ──▶ 6. public-api   (limits become documented contract)
  3. input-bounds ──▶ 5.4 benchmark   (its caps distort the sweep unless neutralized)
  4. observability ──▶ 5.4 benchmark  (benchmark reads the instruments)
  5.0 thread-control finding ──▶ 5.2, 5.4  (can the budget be set at all?)
  7.1 fixture generator ──▶ 5.4         (synthetic model ladder, tunable compute)
```

**Hard dependencies** (violating these causes rework):

- Everything → 1. It changes every call path.
- 5.4 benchmark → 1. Benchmarking blocking code measures a bug.
- 5.4 benchmark → 4. Otherwise the numbers are unreproducible in production.
- 5.4 benchmark → 5.3. Sweeping replica count before the thread budget is fixed confounds the two variables.
  *Held in practice by a different route than expected:* 5.4 ran before 5.3 because 5.0 established
  that `intra_threads` is honored, which is the only part of 5.3 the benchmark actually depended on.
  The rest of 5.3 (pre-packed weights, the global pool) changes memory and startup cost, not the
  replica/thread trade-off, so it does not invalidate the sweep. 5.3b — the measured defaults — is
  now downstream of 5.4 rather than upstream of it.
- 5.4 benchmark → 5.0. If `intra_threads` is inert on our binaries, the fixed-thread-budget comparison cannot be constructed at all — this is a finding, and it must precede the harness.
- 5.4 benchmark → 3. `max_in_flight` and `inference_timeout` will silently distort the sweep unless raised or made explicit axes.
- 6 → 2, 3. The REST surface exposes health and documents limits.
- 2.2 ↔ `prompt-scanner` coverage-reporting Phase 2. Cross-gear, both directions.

**Soft ordering:** 3 and 4 can swap. 7.1 should move as early as convenient. 7.6-7.8 can be
written any time.

## 5. If You Get Blocked

| Blocked on | Jump to | Why it's safe |
|---|---|---|
| Fixture-model authoring harder than expected (7.1) | Stage 1 with feature-gated real-model tests | Stage 1's tests are concurrency tests; they need *a* model, not a small one. Come back to 7.1. |
| `SecurityContext` decision not yours to make (6.3) | 6.1, 6.2, then Stage 7 | serde and the healthcheck are unaffected by the authorization posture. |
| Benchmark refutes the pool sizing (5.4) | Continue to Stage 6 | Record it, set the default from measurement, open a follow-up. Do not redesign mid-chain — and note the pool itself cannot be removed, since ort's `Run` is not thread-safe. |
| Global thread pool needs an application change (5.3) | Finish 5.3's gear-side items, then Stage 6 | Shared weights, the thread-budget log, and the OpenMP finding are all gear-local. The environment wiring can land separately; record it as an open cross-cutting item. |
| `intra_threads` turns out inert on our binaries (5.0) | Wire `OMP_NUM_THREADS` as the real control, then 5.4 | The benchmark still has a thread budget to fix — it is just set from the environment rather than config. What must **not** happen is sweeping replicas with an uncontrolled budget and reporting the result as a pool verdict. |
| Benchmark numbers are too noisy to compare (5.4) | Reduce axes before reducing rigour | Cut the model population to one, or the concurrency levels to three — but keep the input-size ladder and the fixed thread budget. Those two are what make the result mean anything. |
| Cross-gear coverage mapping stalled (2.2) | Stage 3, then 4 | Ship the permissive default *with* the typed error; the consumer-side mapping can land later, but note it as an open cross-gear gap rather than closing 2.2. |
| Tokenizer offsets turn out wrong (7.3) | Stop and escalate | This one is not a "keep moving" case: it affects correctness of every consumer's spans. |

**Never a valid fallback:** starting Stage 5's benchmark before Stage 1. The numbers would
characterize the bug, not the design.

## 5a. What landed outside this plan

Recorded here for the same reason Stage R is: work that happened, was not budgeted, and
changes what the remaining stages are for. All of it is uncommitted at the time of writing.

**Model-selection research (~3d, unbudgeted).** The benchmark harness made it cheap to
measure seven candidate PII models, and that turned into
[QUALITY-EVALUATION.md](../../benchmarks/QUALITY-EVALUATION.md) (the design and survey) and
[QUALITY-RESULTS.md](../../benchmarks/QUALITY-RESULTS.md) (four of seven models scored against
REDACT), plus an evaluation harness that lives in `onnx-scanner`.

**This work does not belong to this gear, and the boundary needs enforcing.**
[DECOMPOSITION.md](./DECOMPOSITION.md) §6 says so directly: "Model selection and detection
quality. A `prompt-scanner` concern. This gear must not acquire an opinion about which model
is good." The performance numbers are legitimately ours — they measure the runner. The
quality numbers measure the *models*, and they are sitting in this gear's docs directory
because that is where the harness was. Moving `QUALITY-*.md` and the per-model quality notes
to `prompt-scanner` is an iteration-1 tidy, not a debate.

**Three gear-local changes came with it**, and these are ours to keep:

- **`LoadedModelInfo.labels`** — the model's `id2label` map, read from a sidecar named by a
  new `labels_file` config key. Serving metadata that sits beside the weights, exactly as
  `tokenizer.json` is served, so it names no model family. Absent or malformed yields `None`
  and never fails a load; a non-contiguous map is rejected rather than padded, because
  consumers index it positionally.
- **`declared_input_names()`** — reads a graph's declared inputs by building a session
  *exactly as the gear does*. Not a convenience: the declared set is not a property of the
  file alone, and `piiranha-v1` reports a different set from a plainly-built session than
  from the gear's. A contract read any other way disagrees with the session that will run.
- **Queue-wait attribution fix** — time spent acquiring a replica is now counted as queueing
  rather than execution. Conflating them inverted the diagnosis: contention read as a slow
  model. This is a correctness fix to Stage 4's instruments, found by Stage 5's sweep, and
  it is the reason the benchmark's numbers are interpretable at all.

**Plus** `encode_word` → `encode_fragment` throughout (the old name sent readers looking for
a word-level API that does not exist), a Docker benchmark bundle, and a one-command remote
runner.

## 6. Estimates

| Stage | Days | State |
|---|---|---|
| 0 — Correct the record | 0.5 | ✅ done |
| R — ONNX Runtime provisioning *(unplanned)* | 1.5 | ✅ done |
| 1 — Execution safety | 4 | ✅ done |
| 2 — Availability | 3 | ◐ done except warmup (~0.5 deferred) |
| 3 — Bounds | 3 | ✅ done |
| 4 — Observability | 2 | ✅ done |
| 5 — Configuration and measurement | 6 | ◐ 5.0, 5.1 + 5.4 done (~4); 5.3a/b and 5.5 remain (~1.25); 5.2 and 5.3's rest triaged |
| 5a — Model-selection research *(unplanned, not this gear's)* | 3 | ✅ done; moves to `prompt-scanner` |
| 6 — Contract and health | 2 | ✅ done |
| 7 — Close out (tests + docs) | 5.5 | ◐ 7.1-7.5 done (~4.5); 7.6-7.9 remain (~1.75) |
| **Total** | **30** | **~25 spent, ~5 remaining for iteration 1** |

Engineer-days, including unit tests and ADRs per stage. Excludes review latency and
everything in [§8](#8-triaged-out-of-iteration-1).

The total grew by the 1.5 unplanned days of Stage R, 0.5 for step 5.0, and 3 for the
model-selection research in [§5a](#5a-what-landed-outside-this-plan) — which was not this
gear's work and is accounted here only because it was done under this plan. Warmup's ~0.5
days is deferred rather than spent, and returns only if the decision it needs is taken.

**The split changed the shape of the remainder more than the size.** Before triage: ~8.5
days, of which 4 were the `execution_provider` field, pre-packed weights and the global
thread pool — three items that add configuration surface. After: ~5 days, of which more than
half is documentation and the rest is test coverage and a healthcheck. Iteration 1 adds
almost no new code paths, which is the property that makes it safe to put out.

**Against the previous plan:** the program's
[DEVELOPMENT_PLAN.md](../../../../docs/genai-security/internal/DEVELOPMENT_PLAN.md) §3 budgets **4 days**
for this gear, as a single `multi-model-execution` feature. That estimate covered the
de-risking work only, and was written when the gear's state was believed to include an
`execution_provider` field, a models endpoint, and a defensible concurrency model. None of
the three held. 4 of the 26 days here are that original feature; the other 21 are the spike debt
it was sitting on.

## 7. Interaction With the Program Plan

Only Stage 5 is on the critical path for Milestone 2's detection work — it is what
`prompt-injection-detection` actually needs (a second model hosted, provider settings real).
Stages 1-4 are the prerequisites that make Stage 5's measurements meaningful and the gear
safe to run under load; Stages 6-7 are release requirements that no M2 feature blocks on.

**M2's remaining dependency on this gear is now discharged on this side.** It was 5.1 alone,
and 5.1 has landed; what is left of that acceptance test is the consumer's own change. The measurement
half of Stage 5 is done, and "provider settings real" turned out not to be what M2 needed:
the benchmark shows CPU is where this workload lives, so the M2 requirement reduces to
"a second model hosted", which is 5.1's external acceptance test — `prompt-injection-detection`
adds a model with configuration changes only. Everything else in iteration 1 is release
hygiene that M2 does not block on.

Two consequences worth deciding explicitly rather than discovering:

1. **The program's M2 can proceed after Stage 5** rather than after the whole chain. With
   Stages 1-3 and R done, that is roughly 7 days out (Stages 4 and 5), not 17. Stages 6-7 can
   run in parallel with `prompt-scanner` work or after M2.
2. **Stage 2 is coupled to the program's largest gap, and the coupling is now live.** The
   permissive-startup default has shipped, so the gear returns `ModelUnavailable` today —
   but until `onnx-scanner` maps it to `Failed` coverage, an unavailable model still reaches
   a scan verdict as an absence of findings. This is no longer a sequencing preference; it
   is an open correctness gap with one half already in `main`'s path. `coverage-reporting`
   Phase 2 is the other half.

The program plan's 4-day line item for this gear should be replaced by a reference to this
document. That edit is not yet applied — it changes M2's headline number, which is a
scheduling decision, not a documentation one.

## 8. Triaged out of iteration 1

Each of these is real work with a real justification. None of them is a safety, honesty or
operability requirement, which is the bar [§2a](#2a-iteration-1-what-actually-ships) applies.
Open one task per row; do not let them re-enter the release by drift.

| Item | Why it is safe to ship without | What would pull it back in |
|---|---|---|
| **5.2 — `execution_provider` field** | There is no non-CPU deployment target, and the benchmark shows the workload is memory-bandwidth-bound, so a GPU is not the obvious win it looks like. A config knob whose non-CPU paths we cannot test is an untested surface that reads as a promise. Iteration 1 makes the docs true by recording CPU-only (ADR 0009) rather than by adding the field. | A deployment target with a GPU, or a model whose profile is compute-bound. |
| **5.3 — shared `PrepackedWeights`** | A memory optimization for a cost we can currently pay. Peak RSS is now measured per configuration, so the cost is visible rather than unknown — which is what made it urgent. | Resident memory becoming the binding constraint on replica count in a real deployment. |
| **5.3 — global thread pool** | Not ours to wire. `ort` states a library must never configure the environment, and the environment cannot be reconfigured after commit, so this belongs to `cf-gears-example-server` / toolkit bootstrap. The gear must merely work correctly either way, which it does. | The application taking the decision, at which point the gear needs a test that it works under both. |
| **2.3 — model warmup** | Needs a decision, not typing: synthetic inference from a contract that declares only tensor *names* would take a working model out of service (see Stage 2). Deferring it costs a first-request latency cliff, which is a performance wart, not a defect. | Per-model warmup inputs in config, or a richer contract. Reopen with that decision, not before. |
| **Porting the feature-gated suites to the fixture** | Concurrency, panic-recovery and cancel-safety all have passing tests; they simply need a real model, so CI compiles them but does not run them. Coverage is thinner than it looks, but nothing is unverified. | Any change to the replica pool or the panic path, which is exactly when the gated suites matter and are not running. |
| **Remaining quality evaluation** (3 of 7 models) | Model selection is `prompt-scanner`'s concern by this gear's own stated boundary. | Nothing here. It moves with the docs. |
| **Open performance questions** — thread spinning, NUMA pinning, GLiNER label-count scaling, end-to-end validation of the 12x | All recorded in [PERFORMANCE-RESEARCH.md §5](../../benchmarks/PERFORMANCE-RESEARCH.md#5-open-questions). The measured knobs are close to exhausted: everything remaining is bounded by the ~1.3x that all runtime tuning is worth against model choice's 12x. | A deployment that needs the last 30%. |
| **3.8 — batch inference, tokenizer truncation/padding, `decode`** | Additive, and the feature file is deliberately unwritten until 3.5 settles the contract shape. Batching measured at +14%. | A consumer with a batch workload. |
| **Cross-gear: `coverage-reporting` Phase 2** | Not this gear's code. The producer side is complete — `ModelUnavailable` with a stable reason, distinct from `UnknownModel`. | Nothing we can do here. It is `prompt-scanner`'s half, and it stays an open correctness gap until landed. Track it there, and keep it named in §7. |

**The one that should worry you most** is the cross-gear coverage mapping. It is the only
row where shipping iteration 1 leaves a *correctness* gap live in the program — an
unavailable model still reaches a scan verdict as an absence of findings. This gear cannot
close it, but it should not be allowed to go quiet.
