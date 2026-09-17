Created:  2026-08-24 by Constructor Tech
Updated:  2026-08-28 by Constructor Tech

# Feature: Execution Configuration and Multi-Model Hosting

<!-- toc -->

- [1. Feature Context](#1-feature-context)
- [2. Current State](#2-current-state)
- [3. Scope](#3-scope)
- [4. Phases](#4-phases)
  - [Phase 1: Verify Concurrent Multi-Model Hosting](#phase-1-verify-concurrent-multi-model-hosting)
  - [Phase 2: Real Execution Settings](#phase-2-real-execution-settings)
  - [Phase 3: Fix the Thread and Memory Model](#phase-3-fix-the-thread-and-memory-model)
  - [Phase 4: Concurrency Benchmark](#phase-4-concurrency-benchmark)
- [5. Feature Acceptance Criteria](#5-feature-acceptance-criteria)
- [6. Non-Functional Considerations](#6-non-functional-considerations)
- [7. Outcome](#7-outcome)

<!-- /toc -->

- [ ] `p1` - **ID**: `cpt-cf-onnx-runner-featstatus-multi-model`

## 1. Feature Context

- [ ] `p1` - `cpt-cf-onnx-runner-feature-multi-model`

### Overview

Make execution settings real and per-model, prove the gear hosts several models
concurrently, and measure whether the session replica pool earns its memory cost.

**Closes**: [DESIGN.md](../../DESIGN.md) §5 item 11. **Validates or refutes** recorded decision
2 (replica pool over a single session).

### Purpose

Two distinct jobs, kept in one feature because the second cannot be measured without the
first.

*Correct the record.* `execution_provider` is documented in three places and implemented
nowhere — the CPU provider is hard-coded in `infra/ort_session.rs`. Until the field exists,
every claim about provider configurability is false, and no non-CPU deployment is possible.

*De-risk the second model.* The gear is configured as though it hosts several models and has
never been run with more than one loaded. Prompt-injection detection needs a second model
alongside the PII one; discovering a runner-level problem in the middle of that work is the
expensive outcome this feature exists to prevent.

**Traces to**: concurrent multi-model execution, `cpt-genaigw-nfr-performance`,
`cpt-cf-onnx-runner-principle-generic-execution`

### References

- **Gear design**: [DESIGN.md](../../DESIGN.md) §2.2, §4 (decisions 2, 3), §5
- **Config**: `config/genai-security.yaml` → `gears.onnx-runner.models`
- **Depends on**: [execution-safety](./execution-safety.md) — benchmarking the current
  blocking implementation would measure the wrong thing;
  [model-lifecycle](./model-lifecycle.md) — per-model health moved there;
  [observability](./observability.md) — the benchmark reads its instruments

## 2. Current State

Working:

- `Vec<ModelConfig>` in config; per-model `replicas` and `intra_threads`.
- Session replica pool per model with round-robin dispatch (7 unit tests on the pure
  selector/verification logic).
- SHA-256 and graph-contract verification at load.

Absent or unverified:

- **`execution_provider` does not exist.** `CPU::default()` is hard-coded. Previous versions
  of this file and of [DECOMPOSITION.md](../DECOMPOSITION.md) claimed the field was present;
  it never was.
- `inter_threads` is hard-coded to 1; optimization level hard-coded to `Level3`.
- Only one model (`gliner2-pii`) is ever loaded; `minilm-l6-pii` is fetchable but not
  configured.
- No test loads two models, and no test exercises concurrent `run()` at all.
- The replica pool has never been benchmarked against a single session. The default of 4
  replicas — 1.7 GB resident for the current 441 MB model — rests on reasoning alone.
- **The thread model is unsound as configured.** Verified against `ort` 2.0.0-rc.12/rc.13
  sources:
  - Each `Session` builds its **own** intra-op thread pool (and inter-op pool under parallel
    execution) unless the environment was created with a global pool. N replicas therefore
    means N weight copies *and* N thread pools; `replicas × intra_threads` is never computed
    or logged.
  - `with_intra_threads` is documented to have **no effect on OpenMP builds** — which is what
    the `download-binaries` feature fetches from Microsoft. `OMP_NUM_THREADS` governs there,
    so the configured `intra_threads: 1` may be doing nothing at all today.
  - `EnvironmentBuilder::with_global_thread_pool` (which triggers `DisablePerSessionThreads`
    at session commit) and `SessionBuilder::with_prepacked_weights` — ORT's own answers to
    shared pools and shared weights across sessions of one model — are both unused.
  - What is *not* fixable this way: sharing a single session across concurrent callers.
    `ort` makes `run` take `&mut self` deliberately because ONNX Runtime's `Run` is not
    thread-safe ([DESIGN.md](../../DESIGN.md) §2.2 quotes the maintainer's comment verbatim).
    Replicas are the only concurrency mechanism available; the question is how to make them
    cheap, not whether to have them.

## 3. Scope

**In scope:**

- `execution_provider` and remaining session-builder settings as real per-model config, with
  validation and documented defaults.
- The thread and memory model: shared weights, shared-vs-per-session pools, the OpenMP
  interaction, and a visible thread budget.
- Verification and fixes for concurrent multi-model hosting.
- A reproducible concurrency benchmark, and a default replica count derived from it.

**Out of scope:**

- Per-model health and degraded startup — [model-lifecycle](./model-lifecycle.md).
- Readiness reporting — [sdk-contract-and-health](./sdk-contract-and-health.md).
- Choosing the prompt-injection model — `prompt-scanner`'s `prompt-injection-detection`
  Phase 1.
- Model-family decoding logic; stays in `onnx-scanner`
  (`cpt-cf-onnx-runner-principle-generic-execution`).
- Validating a specific GPU deployment target. Making the setting real and honest is in
  scope; certifying CUDA or CoreML on a target host is not, until such a target exists.
- Dynamic load/unload at runtime — deliberately deferred.

## 4. Phases

> **Phase status.** Phases 1 and 4 are done. Phase 1 and the cheap half
> of Phase 3 are in iteration 1. **Phase 2 is triaged out** and Phase 3's expensive half with
> it — see [RELEASE_PLAN §8](../RELEASE_PLAN.md#8-triaged-out-of-iteration-1) for the
> reasoning and the conditions that pull each back in. The phases below are left as written;
> a phase is not rewritten because it was deferred.

### Phase 1: Verify Concurrent Multi-Model Hosting

> **Outcome.** Done, and it found no defect — which is worth stating precisely, because
> "no defect" is the weakest possible finding and the easiest to overclaim. What is now
> proven: two graphs that disagree on names, arity and dtypes load side by side with
> independent contracts, digests, tokenizers and replica pools; dispatch by id cannot
> return another model's answer; the three outcomes *served*, *configured-but-broken* and
> *never configured* stay distinct; and 24 interleaved concurrent calls across both models
> each get back the payload they sent. What is **not** proven is anything about memory or
> contention at real-model scale — both fixtures are a few hundred bytes, and a 446 MB
> artifact at four replicas is a different question that belongs to Phase 4's harness.
>
> **The second fixture is the substance of this phase.** Verifying multi-model hosting with
> two copies of one graph would have passed a test that no isolation at all would also
> pass, since every call would succeed whichever model answered. `other.onnx` exists to
> make the wrong answer detectable.
>
> **One thing the tests taught about the gear's own layering:** declaring an output with no
> `output_dtypes` entry fails as `invalid_config` before any session is built, so the first
> attempt at a contract-mismatch test measured Stage 3's config check instead of the graph
> check. That ordering is correct — cheapest validation first — and the test now declares a
> dtype for the phantom output so it reaches the graph.

> **Two different "how many models" questions.** This phase asks whether *several distinct
> models* can be hosted at once — a question about memory, contract isolation, and
> cross-model interference. Phase 4 asks whether *several replicas of one model* buy
> concurrency. They scale memory roughly alike but differ in thread-pool contention and
> cache behaviour, and a benchmark should sweep both: a full matrix for replicas, a smaller
> one for distinct models.

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-phase-multi-model-verify`

Prove the existing design works with N models, and fix what does not.

**Requirements**

1. Two models load simultaneously from config, each with its own replica pool, tokenizer,
   graph contract, and checksum.
2. `run()` and `encode()` dispatch by `model_id`; an unconfigured id is a typed error, never
   a panic or a wrong-model result.
3. `list_models()` returns all configured models with correct per-model metadata.
4. Concurrent calls against *different* models do not serialize against each other.
5. Concurrent calls against the *same* model use distinct replicas up to pool size.

**Acceptance criteria**

- [x] An integration test loads two distinct models (fixture models are fine — see
      [test-suite](./test-suite.md) Phase 1) and asserts both are listed with correct
      `input_names` / `output_names` / `sha256`.
      *(A second committed fixture, `other.onnx`, 307 bytes.)*
- [x] A test asserts an unknown `model_id` returns the typed error, distinct from
      `ModelUnavailable`. *(Three outcomes held apart in one test: served,
      configured-but-broken, never configured.)*
- [x] A test asserts a graph-contract mismatch on the *second* model names which model
      failed, not a generic startup error.
- [x] A concurrency test issues N simultaneous calls across two models and asserts all
      complete with correct per-model outputs. *(24 interleaved calls, each carrying a
      payload unique to it so a crossed response is detectable rather than improbable.)*
- [x] Startup time and peak RSS with two models are recorded. *(Recorded, not asserted:
      80 ms and +29 MB resident at fixture scale on a laptop. The figure an operator
      sizes from needs real artifacts and the benchmark harness.)*
- [x] Any defect found is fixed here, not deferred; an architectural fix becomes its own ADR.
      *(None found. See the outcome below for what that does and does not mean.)*

### Phase 2: Real Execution Settings

- [ ] `p1` - **ID**: `cpt-cf-onnx-runner-phase-exec-settings`

> **Deferred, with the gap closed a different way.** There is no non-CPU deployment target,
> and Phase 4 measured this workload as memory-bandwidth-bound — so a GPU is not the win the
> missing field makes it look like. Shipping `execution_provider` now would add a config
> surface whose non-CPU paths no host available to us can exercise, which is a promise we
> cannot keep. Iteration 1 instead records **CPU-only, deliberately** as ADR 0009, with the
> bandwidth finding as its evidence. That makes the documentation true, which was the actual
> release-bar requirement.
>
> **Reopen when** a deployment target has a GPU, or a model arrives whose profile is
> compute-bound rather than bandwidth-bound.

**Requirements**

1. Per-model config supports `execution_provider`, `replicas`, `intra_threads`,
   `inter_threads`, and `optimization_level` — each actually reaching the session builder.
2. Every setting has a documented default matching today's hard-coded behavior: CPU,
   `intra_threads: 1`, `inter_threads: 1`, `Level3`. Defaults change only as a deliberate,
   visible edit.
3. Validation at load: an unrecognized provider name, a zero replica count, or an
   out-of-range thread count fails startup with a message naming the model and the field.
4. A recognized-but-unavailable provider (CoreML on Linux, CUDA without the runtime) fails
   loudly rather than silently falling back to CPU. Silent fallback makes every subsequent
   benchmark a lie.
5. The total thread budget (`replicas × intra_threads`, plus inter-op) is computed and logged
   at startup, so oversubscription is visible rather than discovered.
6. The CPU-by-default decision and its measured CoreML justification move from a design
   sketch into config comments and the ADR.

**Acceptance criteria**

- [ ] A test asserts each setting is read from config and reaches the session builder.
- [ ] A test asserts invalid values fail startup with a model-naming error, for every
      setting.
- [ ] A test asserts a recognized-but-unavailable provider fails rather than falls back.
- [ ] Defaults are asserted by test.
- [ ] `config/genai-security.yaml` documents every setting with its default.
- [ ] The startup log line states the thread budget.

### Phase 3: Fix the Thread and Memory Model

- [ ] `p1` - **ID**: `cpt-cf-onnx-runner-phase-thread-model`

> **Split by the 2026-09-04 triage.** Requirement 3 (the OpenMP question) is **already
> answered**: `intra_threads` is honored, and the concern died when Stage R replaced the
> OpenMP-linked `download-binaries` with the official dynamic releases. Requirement 2 (thread
> budget computed, logged, oversubscription warned) is **in iteration 1** — it is cheap and
> an operator has no signal today. Requirements 1 and 4 — shared `PrepackedWeights` and the
> environment-level global pool — are **triaged out**: the first is a memory optimization for
> a cost Phase 4 now measures per configuration rather than leaves unknown, and the second is
> not this gear's to make, since `ort` states a library must never configure the environment.

The replica pool is mandatory (see §2 and [DESIGN.md](../../DESIGN.md) §2.2) but is currently
paid for twice: once in weights, once in thread pools. Both costs have ORT-level mitigations
that this gear does not use.

**Requirements**

1. **Shared pre-packed weights.** Replicas of the same model are built with a shared
   `PrepackedWeights` container, so N replicas do not mean N independent copies of the
   pre-packed weight buffers. Measure resident bytes before and after; if the saving is
   negligible for our models, record that and drop it rather than keeping unexplained
   complexity.
2. **Thread budget computed and logged.** `replicas × intra_threads` (plus inter-op, if
   enabled) is calculated at load, logged, and compared against available parallelism.
   Oversubscription produces a `warn` naming the model and the numbers.
3. **The OpenMP interaction is established and documented.** Determine empirically whether
   the binaries we actually ship (`download-binaries`, Microsoft prebuilt) are OpenMP builds.
   If they are, `intra_threads` is inert and the config must say so, with `OMP_NUM_THREADS`
   documented as the real control. Do not leave a config field that silently does nothing.
4. **Global thread pool: decided, not defaulted.** Determine whether replicas should share
   one environment-level pool (`with_global_thread_pool` → `DisablePerSessionThreads`).
   Note the constraint: this is an **environment**-level setting, `ort`'s docs state a
   library must never configure the environment itself, and the environment cannot be
   reconfigured after commit. So the gear can at most *require* or *request* it — the wiring
   belongs to the application (`cf-gears-example-server` / toolkit bootstrap). Whatever is
   decided, the gear must behave correctly under both, and `with_independent_thread_pool` is
   the per-session opt-out if a model needs isolation.
5. Replica count, thread settings, and weight sharing are tunable independently, so Phase 4
   can sweep them separately rather than confounding them.

**Acceptance criteria**

- [ ] Resident bytes for a model at `replicas: N` are measured with and without shared
      pre-packed weights, and the numbers are recorded.
- [ ] Startup logs state the thread budget and warn on oversubscription; asserted in a test.
- [ ] The OpenMP question is answered with evidence, and `intra_threads`' real effect is
      documented — including "none, on these binaries" if that is the answer.
- [ ] The global-thread-pool decision is recorded in the ADR with its ownership boundary
      (gear vs application) stated explicitly.
- [ ] A test asserts the gear works correctly whether or not the environment has a global
      pool.
- [ ] No configuration field remains that has no observable effect.

### Phase 4: Concurrency Benchmark

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-phase-benchmark`

Establish the numbers M4 performance validation will be measured against — and settle
whether the replica pool works.

> **Outcome.** Done. The pool works: at a fixed 48-thread budget, many thin replicas beat one
> fat session by **~4x** under concurrent load. Harness at `src/bin/benchmark.rs`, results
> under [docs/benchmarks/](../../benchmarks/), conclusions in
> [PERFORMANCE-RESEARCH.md](../../benchmarks/PERFORMANCE-RESEARCH.md). Two acceptance criteria
> below remain open and carry into the close-out: the measured default is not yet in shipped
> config, and the ADR is unwritten.
>
> **The gating finding resolved in the permissive direction** — `intra_threads` is honored,
> so the fixed-budget comparison was constructible as specified. Stage R had already removed
> the OpenMP-linked binaries the concern was recorded against.
>
> **Requirement 2's prediction was wrong, and that is the finding.** Input size does not
> decide replicas-versus-threads; **concurrency** does. One caller always favours threads,
> four or more always favour replicas, and input size moves only the margin. Sweeping three
> axes would have produced a confident wrong answer — which is the argument for the fourth
> axis, just not the argument that was made for it.

**Blocked on a finding, not just on code.** Phase 3 requirement 3 asks whether
`with_intra_threads` does anything on the binaries we ship. If the answer is "no, these are
OpenMP builds", then *the thread budget cannot be set through this gear's config at all*,
and the headline comparison below — 4 replicas × 1 thread against 1 replica × 4 threads —
is unmeasurable until `OMP_NUM_THREADS` is wired instead. Establish that first; a negative
answer reshapes what this phase can measure.

**Requirements**

1. A reproducible benchmark: throughput and p50/p95/p99, swept over **four** axes —
   replica count, intra-op thread count, concurrency level, and **input size**. Replicas
   and intra-op threads trade against each other; sweeping only one answers nothing.
2. **Input size is an axis, not a constant.** Replica benefit is a function of it: at small
   inputs, framework overhead dominates and replicas look pointless; at large inputs,
   compute dominates and intra-op threading may win. The crossover point is the finding an
   operator actually needs. Sweeping replicas at a single input size can produce either
   conclusion, and both would be wrong. A size ladder — order-of-magnitude steps, e.g.
   16 / 128 / 512 / 2048 tokens — is the minimum.
3. A fixed, committed payload set at each rung of that ladder — not ad hoc text — so runs
   are comparable over time.
4. **Two model populations, for two different questions.**
   - *Generated fixtures* (per the generator from [test-suite](./test-suite.md)) with
     tunable compute — e.g. `MatMul` over sized initializers. These make compute cost a
     controlled variable independent of artifact size, which is what answers the
     pool-design question cleanly. They are kilobytes, committed, and byte-reproducible.
   - *At least one real model*, for one validation run. A synthetic graph can answer "does
     the pool work"; only a real one answers "does that hold for what we ship".
5. Peak RSS reported per configuration alongside latency. Note this is **not** the
   `replicas × file size` estimate [observability](./observability.md) Phase 1 records at
   load: actual resident memory includes ONNX Runtime's arenas, per-session thread stacks,
   and pre-packed weights. Decide and document how it is measured, cross-platform.
6. **The bounds added by [input-bounds](./input-bounds.md) must be neutralized or made
   explicit axes**, or this phase measures them instead of the design:
   - `max_in_flight` defaults to the replica count, so a concurrency sweep past that
     ceiling measures the semaphore, not contention. Raise it deliberately — or treat it as
     a fifth axis, which is arguably the more useful experiment.
   - `inference_timeout` defaults to 30s. A large input at high concurrency can hit it,
     quietly removing completed work and flattering the latency percentiles.
   - `Overloaded` and `Timeout` responses are counted separately and reported. Throughput
     computed over a denominator that includes shed calls is not throughput.
7. Results committed as a dated document with the hardware described.
8. Runnable from one documented command; **not** part of the default CI run.
9. Timings read from [observability](./observability.md)'s instruments, so the benchmark
   measures the same numbers operators will see, and inference time is separable from queue
   wait. That separation is load-bearing here: the whole point of varying replica count is
   to move work between queueing and execution, and a single total conceals it.
10. **One configuration per process invocation.** Sweeping replica counts means loading and
    dropping many sessions, which is the pattern that intermittently trips ONNX Runtime's
    teardown race (see [test-suite](./test-suite.md) §7). A harness that dies after
    producing numbers is not one anybody will run twice.
11. The harness is a purpose-built binary, not `criterion`. Criterion is built for
    statistically rigorous microbenchmarks of synchronous functions; this is an async
    throughput sweep over a fixed duration with concurrency as an input variable.

**Acceptance criteria**

- [x] The benchmark runs from a single command without manual intervention.
      *(`scripts/run-remote-benchmark.sh` for a remote host; the binary directly for local.)*
- [x] Results show whether the replica pool improves concurrent throughput over a single
      replica, **at a fixed total thread budget** — comparing 4 replicas × 1 thread against
      1 replica × 4 threads, not against 1 replica × 1 thread. The latter comparison would
      flatter the pool by giving it four times the CPU.
      *(Swept at a fixed 48-thread budget on the 48-core host: `1x48` reaches 10.2/s,
      `24x2` reaches 39.4/s.)*
- [x] That comparison is reported **at each rung of the input-size ladder**, and the
      crossover — the input size at or above which one arrangement overtakes the other — is
      stated, or its absence is stated.
      ***Its absence is stated.*** There is no input-size crossover: the winner flips on
      **concurrency**, not size. Reported per rung regardless, which is how the absence was
      established.
- [x] Shed calls (`Overloaded`, `Timeout`) are reported per configuration, so it is visible
      whether a result was produced under load shedding.
- [x] The measured answer to "one fat session or several thin ones" is stated plainly, since
      that is the question the pool design has been assuming an answer to since day one.
      ***Several thin ones***, under concurrency; one fat one for a single caller.
- [x] If the pool shows no benefit, the finding is recorded and the design revisited rather
      than quietly filed. Do not redesign mid-chain; open a follow-up.
      *(Not triggered — the pool showed benefit. The related finding that* was *recorded: the
      extreme is not the optimum. 24 replicas buys 23% over 6 for 3.6x the memory, and is
      slower at long prompts.)*
- [ ] The default replica count is set from measurement, and `config/genai-security.yaml`
      updated to match. **Open** — the config still ships `replicas: 1`, roughly a quarter of
      achievable on a server. Close-out step 5.3b.
- [x] The memory/throughput trade-off is stated explicitly enough for an operator to size a
      deployment from it.
      *(Peak RSS per configuration in every run file; recommended defaults per workload class
      in [PERFORMANCE-RESEARCH.md §4](../../benchmarks/PERFORMANCE-RESEARCH.md#4-configuration-findings).)*

## 5. Feature Acceptance Criteria

- [◐] `execution_provider` exists, is validated, and is honored — the documentation becomes
      true. **Met by the second clause, not the first**: the documentation is made true by
      recording CPU-only as a decision (ADR 0009). The field is deferred — see Phase 2.
- [x] Two or more models host concurrently, verified under concurrent load. *(Phase 1.)*
- [◐] All session settings are per-model configurable, validated, defaulted, and tested.
      **Reduced to the settings that exist**: `replicas`, `intra_threads`, and the bounds from
      [input-bounds](./input-bounds.md). No new knobs ship in iteration 1.
- [x] A committed benchmark validates or refutes the replica-pool design, with a measured
      default replica count. **Validates it** — ~4x for thin replicas at a fixed budget.
      *(The measured default still has to reach shipped config; Phase 4's last open criterion.)*
- [◐] `prompt-injection-detection` can add a second model with **configuration changes only**
      and no `onnx-runner` source change. This is the real test of the feature; if it fails,
      that is a finding about this feature, not about the consumer. **The producer side is
      proven** — Phase 1 adds its second model through `ModelConfig` alone, touching no gear
      source — but the external test is a consumer action in another repo and cannot be
      closed from here.
- [ ] ADRs written: ~~the settings model~~ *(with Phase 2, deferred)*; CPU-only with its
      measurement; the replica-pool outcome.

## 6. Non-Functional Considerations

**Memory.** N replicas × M models multiplies resident model memory, and the current default
multiplies a 441 MB model by four. Phase 3 must report peak RSS per configuration; Phase 2
must make the number visible at startup.

**Threads.** ONNX Runtime's intra-op pool parallelizes within an inference while replicas
parallelize across inferences; both consume cores. `replicas × intra_threads` is the real
budget and it is currently invisible. On a 10-core host the present default is already
oversubscribed. Note that the pool cannot be replaced by session sharing — that is UB per
`ort` — so the tuning question is genuinely "how many replicas, how wide each", and Phase 4
must answer it at a fixed thread budget or the comparison is meaningless.

**Startup time.** Each model adds read, checksum, N session builds, and verification. Record
it with two models; it affects restart and rollout behavior, not just first-run experience.

**Genericity** (`cpt-cf-onnx-runner-principle-generic-execution`). Nothing here may introduce
model-family awareness. The second model being a different family from the first is the
point; if a phase seems to need family-specific code in the runner, that is a design failure
to escalate.

**Benchmark validity.** Do not benchmark before [execution-safety](./execution-safety.md)
lands. Measuring the current blocking implementation would characterize a bug rather than the
design, and the numbers would have to be thrown away.

## 7. Outcome

Written on completion: `gears/onnx-runner/docs/ADR/NNNN-cpt-cf-onnx-runner-adr-execution-configuration.md`
