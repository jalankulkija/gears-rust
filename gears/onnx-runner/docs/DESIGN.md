Created:  2026-08-24 by Constructor Tech
Updated:  2026-08-27 by Constructor Tech

# onnx-runner — Gear Design

<!-- toc -->

- [1. Scope](#1-scope)
  - [1.1 What This Gear Is](#11-what-this-gear-is)
  - [1.2 Current Maturity: Spike](#12-current-maturity-spike)
  - [1.3 In Scope / Out of Scope](#13-in-scope--out-of-scope)
- [2. Architecture](#2-architecture)
  - [2.1 Layers](#21-layers)
  - [2.2 Execution Model](#22-execution-model)
  - [2.3 Model Lifecycle](#23-model-lifecycle)
- [3. Design Elements](#3-design-elements)
  - [3.1 Principles](#31-principles)
  - [3.2 Components](#32-components)
  - [3.3 Contracts](#33-contracts)
  - [3.4 Sequences](#34-sequences)
  - [3.5 Constraints](#35-constraints)
- [4. Recorded Decisions](#4-recorded-decisions)
- [5. Known Gaps](#5-known-gaps)
- [6. Consumers](#6-consumers)

<!-- /toc -->

## 1. Scope

### 1.1 What This Gear Is

`onnx-runner` executes ONNX models. It owns model lifecycle (load, verify, host), session
management, and HuggingFace tokenization, and exposes them as "run this named model with
these named tensors, get these named tensors back."

It knows **tensors and tokenizers, never model families**. Prompt construction, output
decoding, label sets, and thresholds live in consumers. That boundary is the gear's
defining property: it is what makes the gear reusable for embeddings, classification, or
ranking rather than being a PII-detection component with a generic name.

This document owns the gear: its architecture, its contracts, and the reasoning behind
them. Nothing here is specific to any one consumer.

### 1.2 Current Maturity: Spike

**This gear is a spike.** It was written to establish that Rust-native ONNX execution was
viable, and it does work. It has never been operated, load-tested, or run with more than one
model loaded.

Concretely:

| | |
|---|---|
| Implementation | 1,477 lines across 14 files |
| Tests | 42 in the default suite, plus 14 behind `real-model-tests` |
| Tests covering `infra/` | 24 by default (load, verification, inference, dtypes, tokenizer) plus the model-backed ones |
| Integration tests | 5 suites, run against a committed 285-byte ONNX fixture |
| Models ever loaded simultaneously | 2, in the concurrency test |
| ADRs | 8 |

One claim elsewhere in the doc set was false and is corrected here: there is **no**
`execution_provider` config field — CPU is hard-coded in `infra/ort_session.rs`. §5 lists
the gaps; DESIGN.md turns them into prioritized features.

The reference for what "done" looks like in this repo is `file-parser`'s
`infra/magika_detector.rs` — the same problem (an ONNX session behind an async trait),
solved to production standard, in this codebase, with tests.

### 1.3 In Scope / Out of Scope

**In scope:** model loading and integrity verification; session hosting and concurrency;
execution-provider and threading configuration; tensor marshalling; tokenization; model
introspection and readiness; the in-process client contract.

**Out of scope:** model-family logic (consumer's job); model *selection* and quality
evaluation (a detection concern, not a runtime concern); model downloading in production;
training or fine-tuning; serving non-ONNX formats; **any HTTP surface of its own** — see
§3.5.

## 2. Architecture

### 2.1 Layers

```
  consumer (onnx-scanner, or any gear)
        │  OnnxRunnerClientV1  (ClientHub, in-process)
        ▼
  ┌──────────────────────────────────────────────────────┐
  │ gear.rs        OnnxRunnerLocalClient                 │
  │                BTreeMap<model_id, LoadedEntry>       │
  ├──────────────────────────────────────────────────────┤
  │ domain/        verify_contract, verify_checksum,      │
  │                ReplicaSelector — pure, tested         │
  ├──────────────────────────────────────────────────────┤
  │ infra/         ort_session.rs   ← the only `ort` user │
  │                tokenizer.rs     ← the only            │
  │                                   `tokenizers` user   │
  └──────────────────────────────────────────────────────┘
        │
        ▼  files on disk, placed out-of-band
  models/<model_id>/{model.onnx, tokenizer.json}
```

The single-importer rule for `ort` and `tokenizers` is load-bearing: it is what keeps a
dependency upgrade (and `ort` 2.x churns across release candidates) to one file.

### 2.2 Execution Model

`ort`'s `Session::run` takes `&mut self`, so every inference needs exclusive access to a
session. The gear therefore hosts **N session replicas per model**, each behind its own
mutex, dispatched round-robin by `ReplicaSelector`. A single shared session would serialize
every concurrent request onto one lock.

**The `&mut self` is deliberate, not incidental.** `ort`'s internal `run_inner` takes
`&self`, and `Session` is `unsafe impl Send + Sync` — so it *looks* shareable. It is not.
From `ort-2.0.0-rc.13/src/session/mod.rs:296`:

> Yes, the internal run functions take `&self`. In a perfect world, the public functions
> would take `&self` too. But we don't live in a perfect world, and ONNX Runtime's own
> `Run` is **not thread safe** (despite Microsoft saying it should be years ago!). You may
> modify those functions to take `&self` and run your models from a bunch of threads
> without a care in the world, and everything may *seem* fine, but that's the beauty of
> undefined behavior — it's undefined!

So sharing one session across concurrent callers is not a tuning choice; it is UB. Replicas
are the only concurrency mechanism `ort` 2.x offers, and the mutex per replica is required.

**What the replica pool actually costs.** Each `Session` gets its own intra-op thread pool
(and inter-op pool, if parallel execution is on) unless the *environment* was created with a
global pool. So N replicas means N weight copies **and** N thread pools. Two consequences
the current config gets wrong, both addressed in
execution configuration Phase 3:

- `replicas: 4, intra_threads: 1` forgoes intra-op parallelism *and* pays 4× memory — the
  worst corner of the trade-off space.
- `with_intra_threads` is documented as **having no effect on OpenMP builds** — which is
  what `download-binaries` fetches from Microsoft. `OMP_NUM_THREADS` governs there instead,
  so `intra_threads` may currently be doing nothing at all.

Mitigations exist and are unused: `EnvironmentBuilder::with_global_thread_pool` (which makes
session commit call `DisablePerSessionThreads`) shares one pool across replicas, and
`SessionBuilder::with_prepacked_weights` shares pre-packed weights between sessions of the
same model, which is ORT's own answer to the N-copies memory cost. The global pool is an
**environment**-level setting, and `ort`'s own docs say a library should *never* configure
the environment — so that half is an application/toolkit decision, not this gear's.

**This is where the spike is weakest, part two.** The surrounding async behavior is also
wrong: `run()` and `encode()` are `async fn`s that do CPU-bound work and take a
`std::sync::Mutex` while executing on a Tokio worker thread. Under concurrency this occupies
the async runtime's workers with blocking work and degrades every other task in the process,
not just this gear. The target design offloads inference to the blocking pool and queues on
an async mutex — exactly the pattern `file-parser` documents. See
execution safety.

### 2.3 Model Lifecycle

Today: all configured models load eagerly in `Gear::init`, each replica built one at a time;
any failure aborts gear init and therefore server startup. There is no unload, no reload, no
readiness state, and no warmup — the first real request pays first-inference cost.

Target: per-model load outcome recorded as state, a model that fails to load marked
unavailable rather than fatal, readiness observable, and load failure reported to callers as
a typed "model unavailable" rather than as an absent gear. Deliberate non-goal: caller-driven
loading of arbitrary paths (§3.5).

## 3. Design Elements

### 3.1 Principles

- [ ] `p1` - **ID**: `cpt-cf-onnx-runner-principle-generic-execution`

  Tensors and tokenizers, never model families. If a change requires this gear to know what
  a model *means*, the design is wrong — push it to the consumer.

- [ ] `p1` - **ID**: `cpt-cf-onnx-runner-principle-fail-at-load`

  Integrity and contract violations surface at load, not at inference. A swapped model file
  or a renamed tensor fails startup loudly rather than silently producing wrong numbers at
  2am. Corollary: nothing about a model may be *guessed* at runtime — see the
  `output_dtypes` defaulting gap in §5.

- [ ] `p1` - **ID**: `cpt-cf-onnx-runner-principle-no-blocking-on-async`

  CPU-bound inference never runs on an async worker thread. Queueing is async and fair;
  execution is on the blocking pool; dropping the future never strands a session.

- [ ] `p1` - **ID**: `cpt-cf-onnx-runner-principle-bounded-work`

  Every request is bounded: input size, tensor count, inference time, and in-flight
  concurrency. An unbounded caller must not be able to exhaust host memory or occupy every
  replica indefinitely.

- [ ] `p1` - **ID**: `cpt-cf-onnx-runner-principle-no-payload-in-telemetry`

  Tensor contents and tokenized text never enter logs, metrics, traces, or errors. Shapes,
  counts, durations, and model ids are safe; the data is not. Inherited from
  `cpt-cf-genai-security-principle-no-raw-content` and it applies to any consumer, not only
  the scanning one.

### 3.2 Components

- [ ] `p1` - **ID**: `cpt-cf-onnx-runner-component-local-client`

  `OnnxRunnerLocalClient` (`gear.rs`) — the `ClientHub`-registered implementation of
  `OnnxRunnerClientV1`. Holds the loaded model map; dispatches by `model_id`.

- [ ] `p1` - **ID**: `cpt-cf-onnx-runner-component-model-registry`

  `domain/model_registry.rs` — pure logic: graph-contract verification (order-independent
  input/output name matching), checksum comparison, round-robin replica selection. The only
  tested code in the gear today.

- [ ] `p1` - **ID**: `cpt-cf-onnx-runner-component-ort-session`

  `infra/ort_session.rs` — the only `ort` importer. Session construction, SHA-256 and graph
  verification at load, tensor marshalling in and typed extraction out, replica dispatch.

- [ ] `p1` - **ID**: `cpt-cf-onnx-runner-component-tokenizer`

  `infra/tokenizer.rs` — the only `tokenizers` importer. `encode` (with special tokens,
  offsets, masks) and `encode_fragment` (no special tokens, ids only). No `decode`, no batch, no
  configured truncation.

  `list_models` also serves the model's `id2label` map as `LoadedModelInfo.labels`, read
  from the file named by `labels_file` (default `config.json`). This is sidecar metadata in
  the model directory, exactly as `tokenizer.json` is, so serving it names no model family;
  interpreting it — BIO merging, taxonomies, thresholds — stays with the consumer. Absent or
  malformed metadata yields `None` and never fails a load, and a non-contiguous map is
  rejected rather than padded, because consumers index it positionally by class id.

### 3.3 Contracts

- [ ] `p1` - **ID**: `cpt-cf-onnx-runner-contract-client-v1`

  `OnnxRunnerClientV1` — `run`, `encode`, `encode_fragment`, `list_models`. Models are
  operator-configured at startup and addressed by id; no path is ever caller-supplied.
  Versioned in the name: a breaking change is `V2`, not an edit.

  A full operator configuration example, covering every knob with its default, is
  maintained alongside this document at [`config-example.yaml`](config-example.yaml).

  Gaps against a v1-worthy contract: no batch variant, no per-call deadline, no typed
  "model unavailable" distinct from "unknown model", and the SDK types carry no
  `Serialize`/`Deserialize`, which blocks any out-of-process transport.

- [ ] `p1` - **ID**: `cpt-cf-onnx-runner-contract-model-config`

  `ModelConfig` — `model_id`, `model_dir`, `model_file`, `tokenizer_file`, `sha256`,
  `skip_checksum`, `expected_inputs`, `expected_outputs`, `output_dtypes`, `replicas`,
  `intra_threads`. `deny_unknown_fields`, so a typo in YAML fails startup.

  Absent and documented-as-present elsewhere: `execution_provider`. Also absent: inter-op
  threads, per-model limits, deadlines, warmup, and optimization level.

### 3.4 Sequences

- [ ] `p1` - **ID**: `cpt-cf-onnx-runner-seq-load` — config → read file → sha256 → build N
  sessions → verify graph contract on replica 0 → load tokenizer → register
- [ ] `p1` - **ID**: `cpt-cf-onnx-runner-seq-run` — `run(model_id, tensors)` → lookup →
  select replica → lock → marshal in → `Session::run` → typed extract out
- [ ] `p1` - **ID**: `cpt-cf-onnx-runner-seq-encode` — `encode(model_id, text)` → lookup →
  tokenize → ids + mask + offsets + special-token mask
- [ ] `p2` - **ID**: `cpt-cf-onnx-runner-seq-degraded-load` — *target:* model fails to load →
  recorded unavailable with reason → gear starts → calls to that id fail typed → health
  reports it

### 3.5 Constraints

- [ ] `p1` - **ID**: `cpt-cf-onnx-runner-constraint-no-caller-paths`

  A caller names a `model_id`; it never supplies a filesystem path, URL, or model bytes.
  This removes path traversal, arbitrary-file-read, and unbounded-memory vectors in one
  stroke, and it is why dynamic model loading is a non-goal rather than a missing feature.

- [ ] `p1` - **ID**: `cpt-cf-onnx-runner-constraint-no-fetch-in-prod`

  The gear loads and verifies files already on disk. `scripts/fetch-models.sh` is dev/CI
  convenience, never invoked at startup or on the request path.

- [ ] `p1` - **ID**: `cpt-cf-onnx-runner-constraint-no-http-surface`

  **This gear exposes no HTTP API of its own.** It is an in-process compute primitive
  reached through `ClientHub`, in the same category as `authn-resolver`, `authz-resolver`,
  `tenant-resolver`, and `cluster` — none of which have a REST surface either. It owns no
  tenant-facing resource, so there is nothing for an endpoint to address. Operator-visible
  readiness belongs in the platform's `Healthcheck` registry (`libs/toolkit/healthcheck.rs`,
  aggregated by API Gateway into `/health` and `/readyz`), not in a bespoke endpoint. If a
  genuine out-of-process consumer ever appears, the answer is the OoP/gRPC pattern
  (`docs/toolkit_unified_system/09_oop_grpc_sdk_pattern.md`), not JSON tensors over HTTP.

- [ ] `p1` - **ID**: `cpt-cf-onnx-runner-constraint-single-importer`

  Only `infra/ort_session.rs` imports `ort`; only `infra/tokenizer.rs` imports `tokenizers`.
  Domain code stays free of both.

- [ ] `p2` - **ID**: `cpt-cf-onnx-runner-constraint-model-artifacts-not-vcs`

  Model files are gitignored (`/gears/onnx-runner/models/`) and fetched by pinned revision
  with checksums. The two current artifacts total 528 MB; nothing that size enters version
  control.

## 4. Recorded Decisions

Carried from the original design sketch. These are the arguments worth keeping. Three are now
recorded as ADRs; the rest are written when the work they describe lands
(DESIGN.md §5).

1. **Generic tensor layer, not model-family-aware.** The rejected alternative was
   configurable "GLiNER mode" / "BIO mode" model kinds. That couples a supposedly generic
   gear to specific architectures, and the next family needs a gear change instead of a
   consumer change. → [ADR 0001](./ADR/0001-cpt-cf-onnx-runner-adr-generic-tensor-layer.md)
2. **Session replica pool, not a single `Mutex<Session>`.** Decided upfront rather than
   deferred, because a single mutex serializes a concurrent request path. Verified as the
   *only* option: `ort` makes `Session::run` take `&mut self` deliberately because ONNX
   Runtime's `Run` is not thread-safe, so sharing one session concurrently is UB, not a
   trade-off (§2.2).
   **The sizing is validated by measurement.** At a fixed total thread budget, many thin
   replicas sustain roughly four times the concurrent throughput of one wide session: on a
   48-core, two-socket host, one session with 48 intra-op threads reaches 10.2 inferences/s
   where 24 replicas of 2 threads reach 39.4. A single session cannot spread 48 threads
   across two sockets. → [ADR 0009](./ADR/0009-cpt-cf-onnx-runner-adr-replica-pool-sizing.md)

   The trade the operator makes is memory. The pool costs N weight copies *plus* N intra-op
   thread pools, and the extreme is not the optimum: 24 replicas buy 23% more throughput
   than 6 for 3.6x the memory, and are *slower* at long prompts. `PrepackedWeights` and an
   environment-level shared thread pool would cut both costs and are unused; the latter is
   an application-level decision, because `ort` requires that a library never configure the
   environment.
3. **CPU execution provider, and only CPU.** CoreML was *measured* as a regression on Apple
   hardware (dynamic shapes and cast/dequantize patterns it cannot fuse), and the workload
   is memory-bandwidth-bound rather than compute-bound — the best measured arrangement
   reaches about a third of what linear scaling from single-thread performance would
   predict, and additional cores buy little. There is therefore no configurable
   `execution_provider`: the provider is hard-coded, deliberately, rather than offered as a
   knob whose non-CPU paths are untested.
   → [ADR 0010](./ADR/0010-cpt-cf-onnx-runner-adr-cpu-only-execution.md)
4. **Model provisioning out-of-band.** Pinned HF revision plus sha256 in a dev script;
   operators place files in production. No floating `latest`, ever.
   → [ADR 0002](./ADR/0002-cpt-cf-onnx-runner-adr-out-of-band-model-provisioning.md)
5. **Eager load at startup.** Fail fast on a bad artifact rather than at first request. The
   part that needs revisiting is blast radius: one bad model currently takes down the
   process.
6. **`encode_fragment` as a distinct primitive.** "Tokenize without special tokens" is generic,
   even though the need arose from a GLiNER-family consumer building a schema prefix. It
   stays because it is expressible without naming a model family. Named for the *document
   versus fragment* distinction it actually draws; it was `encode_word` until 2026-09-01,
   which sent readers looking for a word-level API that does not exist.

7. **Model loading is bounded and runs off the async runtime.** The workspace pins `ort`
   with `load-dynamic`, so ONNX Runtime is resolved from `ORT_DYLIB_PATH` at process
   start; `scripts/fetch-onnxruntime.sh` provisions it for dev and CI alike. This gear
   deliberately does **not** enable `ort`'s `download-binaries`/`copy-dylibs`: those link
   statically, and because Cargo unifies features workspace-wide they cannot coexist with
   `load-dynamic` — the combination is what made this gear deadlock where `file-parser`
   did not. When the library is missing, unset, or older than the pinned `api-24` ABI,
   `ort` 2.0.0-rc.12 **hangs instead of erroring** — reproduced here by stack sampling,
   and independently hit by `file-parser`. The mechanism lives in `libs/ort-bootstrap`,
   shared by both gears rather than reimplemented in each: a raw `std::thread` (not
   `spawn_blocking`, which Tokio joins on shutdown) behind a `oneshot`, under a timeout —
   one minute per configured session here — racing the gear's cancellation token. A
   timeout abandons that thread, which obliges the caller to terminate the process;
   `BootstrapError::leaked_init_thread` makes that checkable rather than merely
   documented. Running off the runtime is right for a second reason: loading reads and
   hashes the whole model file and builds one session per replica.

## 5. Known Gaps

Ordered by severity. DESIGN.md maps each to a feature.

**Blocking release:**

1. ✅ **Blocking work on async workers.** *Closed.* `run`/`encode` execute inside
   `tokio::task::spawn_blocking`, replicas are guarded by `tokio::sync::Mutex` with
   `lock_owned`, and no `std::sync::Mutex` remains on a request path (guarded by a test).
   Model loading also moved off the runtime — see decision 7.
2. ✅ **No recovery from a poisoned session.** *Closed.* Each replica slot holds an
   `Option<Session>`; the blocking task takes the session and puts it back, so an unwind
   leaves the slot empty rather than poisoned, and the next caller rebuilds it from the
   same verified spec at the same threading settings.
3. ✅ **`ReplicaSelector::next_index` asserts.** *Closed.* The replica count is a
   `NonZeroUsize` fixed at construction, so the assert is gone and zero is
   unrepresentable. `replicas: 0` is now rejected at load, before any I/O, with an error
   naming the model and the field.
4. ✅ **One bad model kills the process.** *Closed.* Every configured model is held with
   a state; one that fails to load is recorded `Unavailable` with a stable
   `ModelFailureReason` and the gear still starts. Callers get `ModelUnavailable`,
   distinct from `UnknownModel`. `require_all_models` restores fail-fast for deployments
   that want it. A model that loses every replica and cannot rebuild transitions to
   `Unavailable` at runtime rather than failing every call opaquely.
5. ✅ **No bounds.** *Closed.* Per-model caps on tensor elements, tensor count, dimension
   extent and text length; a `30s` inference deadline returning `Timeout`; and an in-flight
   ceiling defaulting to the replica count, shedding as `Overloaded`. The ceiling is what
   bounds this gear's use of the process-wide blocking pool.
6. ✅ **Shape/data consistency unchecked.** *Closed.* Declared shape is checked against the
   element count with `checked_mul`, before any allocation, returning a typed
   `InvalidInput` naming the tensor. Input names are verified against the model's contract.
7. ✅ **`output_dtypes` silently defaults to F32.** *Closed.* Every declared output must
   have a dtype — checked before any I/O — and the declaration is verified against the
   graph's own output metadata at load. The runtime default is gone.
8. ✅ **`infra/` is untested.** *Closed.* A committed 285-byte ONNX fixture, generated by
   `examples/generate_test_fixture.rs`, means the default suite loads a real session with
   no download: checksum verification end to end, contract mismatch on inputs and outputs,
   dtype disagreement caught at load, inference round-trip on both dtypes, replica pool
   behaviour, and tokenization. **Tokenizer offsets are proven to be byte offsets** across
   Cyrillic, CJK and combining sequences — the written warning is now a pointer to the
   tests. The concurrency, panic-recovery and cancel-safety suites still need a real model
   and stay feature-gated, though CI now compiles them.

**Blocking "general purpose":**

9. ◐ **No readiness signal for operators.** Readiness is now derivable through the client
   contract — `ModelInfo::state` plus a live replica count, both read at call time. The
   *operator-facing* projection is still missing: the gear implements no platform
   `Healthcheck`, so `/readyz` and `/health` still say nothing about model availability.
10. **SDK types are not serializable** — no `Serialize`/`Deserialize`, so no
    out-of-process/gRPC transport is possible.
11. **`execution_provider` does not exist.** Documented in three places, implemented
    nowhere; CPU is hard-coded. Also missing: inter-op threads, optimization level. And the
    thread/memory model is unsound as configured: `intra_threads` is a no-op on the OpenMP
    builds `download-binaries` fetches, `replicas × intra_threads` is never computed, and
    neither `PrepackedWeights` nor a global thread pool is used (§2.2).
12. ✅ **`SecurityContext` is accepted and ignored** on every method. *Closed by stating the
    position rather than by enforcing.* This gear is a trusted in-process compute primitive: it
    owns no tenant-scoped resource, a caller can never name a path, and there is no policy
    source to consult, so `ctx` is carried for trace propagation and the trait says so in its
    own documentation — including that a successful call is not evidence the subject was
    permitted anything. → [ADR 0011](./ADR/0011-cpt-cf-onnx-runner-adr-security-context-position.md)
13. ✅ **No metrics, no spans.** *Closed.* Nine instruments behind a domain-owned port with
    an OpenTelemetry adapter: queue wait and inference duration as **separate** histograms
    (so contention is distinguishable from cost), tokenize and load durations, an estimated
    resident-bytes figure, call/load/rebuild counters, and an in-flight gauge. Spans cover
    both the client dispatch and session layers. Payload content reaching telemetry fails a
    sentinel test whose teeth were verified against the real code.
14. **No batch inference.** A general-purpose executor that cannot batch leaves most of the
    available throughput unclaimed.
15. **No tokenizer truncation/padding policy.** Long input behavior is undefined at this
    layer and silently pushed to consumers.
16. **`ort` is pinned to a release candidate** (`=2.0.0-rc.12`, forced by `magika`'s hard
    pin elsewhere in the workspace). Shipping a published gear on a pre-release dependency is
    a deliberate decision that needs recording, plus a note on onnxruntime binary
    distribution and licensing.
17. **No PRD, no ADRs, a two-line README, no `GEARS.md` entry.**

## 6. Consumers

| Consumer | Uses | Notes |
|---|---|---|
| `onnx-scanner` (plugin, `prompt-scanner`) | `run`, `encode`, `encode_fragment` | Only consumer today. Owns GLiNER2 prompt building and span decoding. |
| Any future embedding / classification / ranking gear | `run`, `encode` | The reason the generic boundary is worth defending. |

A consumer needs both crates: `onnx-runner-sdk` for the trait and types, and `onnx-runner`
for `#[toolkit::gear(deps=[onnx_runner])]` to resolve at compile time.
