---
status: accepted
date: 2026-08-27
decision-makers: onnx-runner gear maintainers
---

# A model that fails to load is unavailable, not fatal — and says so in a typed error

**ID**: `cpt-cf-onnx-runner-adr-degraded-startup`

<!-- toc -->

- [Context and Problem Statement](#context-and-problem-statement)
- [Decision Drivers](#decision-drivers)
- [Considered Options](#considered-options)
- [Decision Outcome](#decision-outcome)
  - [Consequences](#consequences)
  - [Confirmation](#confirmation)
- [Pros and Cons of the Options](#pros-and-cons-of-the-options)
  - [Permissive default with a typed unavailable error](#permissive-default-with-a-typed-unavailable-error)
  - [Fail fast on any load failure](#fail-fast-on-any-load-failure)
  - [Permissive default without a typed error](#permissive-default-without-a-typed-error)
  - [Lazy load on first use](#lazy-load-on-first-use)
- [More Information](#more-information)
- [Traceability](#traceability)

<!-- /toc -->

## Context and Problem Statement

`Gear::init` looped over configured models and returned an error on the first failure. An
unreadable file, a checksum mismatch after a model bump, a renamed tensor, a malformed
`tokenizer.json` — any one of them aborted gear init, which aborts server startup, which
takes down every other gear in the process.

There was also no per-model state anywhere. A model was either present in the map or the
gear did not exist, so `list_models` reported what loaded with no notion of what *should*
have loaded, and `UnknownModel` conflated "no such model configured" with "configured but
unavailable" — a caller could not tell a typo from an outage.

The question is not only whether to keep starting after a failed load. It is what a
consumer is owed when a model it depends on is missing, because a scanning platform that
cannot distinguish "scanned, found nothing" from "could not scan" reports clean results for
traffic it never inspected.

## Decision Drivers

* This gear shares a process with every other gear. A fatal load turns one corrupted
  download into a total outage of unrelated functionality — an unacceptable blast radius
  for a shared compute primitive.
* `prompt-scanner`'s coverage model depends on distinguishing an unavailable backend from a
  clean scan (`cpt-genaigw-fr-resilient-service`, via `onnx-scanner`). A gear that either
  works entirely or refuses to start gives the consumer nothing to report, because there is
  no running service to ask.
* Model artifacts are large third-party downloads placed out of band. Partial or corrupted
  provisioning is an operational reality, not an exceptional case.
* Failure reasons reach logs, telemetry, and consumer branching, so they must be stable
  classifications carrying no payload data
  (`cpt-cf-onnx-runner-principle-no-payload-in-telemetry`).
* Some deployments genuinely would rather not start than start degraded. That is a
  legitimate posture and should be configurable, not argued about.

## Considered Options

* Permissive default, with a typed `ModelUnavailable` error
* Fail fast on any load failure (the status quo)
* Permissive default, without a typed error
* Lazy load on first use

## Decision Outcome

Chosen option: **permissive default with a typed unavailable error**. A model that fails to
load is recorded `Unavailable` with a stable reason and the gear starts; `require_all_models`
(default `false`) restores fail-fast for deployments that prefer it.

**The default and the typed error are one decision, not two.** Starting degraded is only
defensible because the degradation is *reported*: callers get
`OnnxRunnerError::ModelUnavailable { model_id, reason }`, distinct from `UnknownModel`, and
`list_models` reports every configured model with its state. Shipping the permissive default
without the typed error would trade a loud failure for a silent one — a scanner would return
no findings for a model that never loaded, which is worse than not starting at all. Anyone
revisiting the default must treat the pair as a unit.

`ModelFailureReason` is a **fieldless** enum — `FileUnreadable`, `ChecksumMismatch`,
`ContractMismatch`, `TokenizerInvalid`, `SessionBuildFailed`, `InvalidConfig` — classified
at the point the cause is known. Making it fieldless is what makes the payload-free
guarantee hold by construction rather than by review: there is nowhere for a path or a
tensor to go. Detail reaches the operator through a `warn` log at the failure site, and
never leaves the process.

### Consequences

* `ModelInfo` gained `state`, and its load-only fields moved into
  `loaded: Option<LoadedModelInfo>`. Separate rather than empty-when-unavailable, so a
  consumer cannot mistake "this model declares no outputs" for "this model never loaded".
  This is a breaking change to the SDK type; the only in-tree consumer was a test mock.
* State and live replica count are read at call time, not frozen at load. A model that loses
  every replica to panicking inferences (see
  [ADR 0004](./0004-cpt-cf-onnx-runner-adr-blocking-pool-offload.md)) and then fails to
  rebuild transitions to `Unavailable { SessionBuildFailed }` and reports itself so, instead
  of failing every subsequent call with a fresh rebuild error.
* **That transition is one-way.** Nothing in this gear can tell whether the underlying cause
  was fixed, so it does not attempt recovery. Recovery is a restart, or the runtime reload
  this design deliberately defers. A future reload feature drives exactly this state
  machine, which is why the state is explicit rather than implied by map membership.
* Zero successfully loaded models is not a startup failure under the permissive default, but
  it is logged at `error`. A gear serving nothing is almost never intended, and it must not
  be something an operator discovers from a consumer's coverage report.
* **The consumer half is not done.** `onnx-scanner` does not yet map `ModelUnavailable` to
  `Failed` coverage — that is `prompt-scanner`'s `coverage-reporting` Phase 2. Until it
  lands, an unavailable model still reaches a scan verdict as an absence of findings. This
  ADR makes that mapping *possible* and does not make it *true*; the cross-gear obligation
  stays open in model lifecycle and availability §5.
* Warmup, specified alongside this feature, is deliberately not implemented — see
  model lifecycle and availability §7. Its spec requires synthetic inputs
  derived from a contract that declares only tensor names, and requires warmup failure to
  mark a model unavailable; together those can take a *working* model out of service.
* Per-model memory (`replicas × model size`) is still not logged, and replicas still load
  serially. Both are named in the feature's non-functional section and both belong with the
  benchmark harness.

### Confirmation

Five tests, each driven through the real `list_models` projection rather than internal
state:

* One good and one broken model: the gear starts, serves the good one, reports the broken
  one `Unavailable { FileUnreadable }`.
* Every failure mode produces its own reason — induced by a missing file, a wrong checksum,
  a renamed output tensor, a corrupt tokenizer, and zero replicas.
* `require_all_models: true` fails startup on the same config, naming the model.
* A typo'd id yields `UnknownModel` while a broken model yields `ModelUnavailable` — the
  distinction a consumer branches on.
* Zero ready models still starts.

The payload-free property needs no test: `ModelFailureReason` has no fields.

## Pros and Cons of the Options

### Permissive default with a typed unavailable error

Chosen. Record the failure, keep serving what loaded, tell callers precisely what happened.

* Good, because one bad artifact cannot take down unrelated gears sharing the process
* Good, because it gives the consumer the three-way distinction it needs — unknown,
  unavailable, inference-failed — which is what makes degraded coverage reportable at all
* Good, because it matches how the gear already handles a lost session: degrade the model,
  keep the gear
* Good, because the state machine it introduces is what a future reload feature would drive
* Neutral, because it needs an escape hatch for stricter deployments, which is one config
  field
* Bad, because a misconfigured deployment can now start and look healthy while serving
  nothing — mitigated by the `error` log, and properly fixed only by the platform
  healthcheck in the SDK contract and health work
* Bad, because the safety of the default depends on a consumer that has not been written
  yet; until `onnx-scanner` maps the error, the honest description is "possible to report",
  not "reported"

### Fail fast on any load failure

The status quo: any load failure aborts init.

* Good, because a misconfiguration is impossible to miss — the process does not start
* Good, because there is no partial state to reason about, in this gear or in consumers
* Neutral, because it is what many single-purpose services reasonably do
* Bad, because this gear is not a single-purpose service; it is one of many in a shared
  process, and this converts its failure into everyone's
* Bad, because it removes the consumer's ability to report degraded coverage: a gear that
  will not start cannot tell anyone why
* Bad, because it makes routine operational events — a partial download, a stale checksum
  after a model bump — into total outages

### Permissive default without a typed error

Start degraded, and let calls to a missing model fall through as `UnknownModel`.

* Good, because it is the smallest change that stops the process dying
* Bad, because it is the worst outcome available: a scanner sees the same error it would see
  for a typo, most plausibly handles it as a configuration mistake, and reports no findings
  for traffic that was never inspected
* Bad, because it converts a loud failure into a silent one — the specific defect the
  program's largest gap is about
* Bad, because the information needed to do better exists at the failure site and is simply
  thrown away

### Lazy load on first use

Defer loading until a model is first requested.

* Good, because startup is fast and unloaded models cost nothing
* Good, because a model fixed on disk becomes usable without a restart
* Neutral, because it would need the same per-model state machine this decision introduces
* Bad, because it contradicts `cpt-cf-onnx-runner-principle-fail-at-load`: a bad artifact
  would surface as a failed *request*, in production, instead of at startup where an
  operator is watching
* Bad, because the first caller pays a multi-second load, and concurrent first callers need
  a load-once guard — real complexity for a benefit an operator does not want
* Bad, because it makes readiness meaningless: nothing is ready until someone asks

## More Information

The permissive default is the second half of a cross-gear decision recorded in the program's
release plan: the plan warned that `require_all_models` defaulting to permissive obliges
`onnx-scanner` to map an unavailable model to `Failed` coverage, and that "neither side is
done alone". That warning is why the consumer-side gap is recorded here as an open
obligation rather than quietly assumed. The producer side is complete; the pair is not.

One implementation note worth keeping: classification happens where the cause is known — the
`std::fs::read` error site produces `FileUnreadable`, the checksum comparison produces
`ChecksumMismatch`, and so on. Classifying later, from an error string, would be exactly the
kind of stringly-typed inference this gear's graph-contract verification exists to reject.

## Traceability

- **DESIGN**: [DESIGN.md](../DESIGN.md) §2.3, §3.4, §5 item 4

This decision directly addresses the following requirements or design elements:

* `cpt-cf-onnx-runner-seq-degraded-load` — this ADR is that sequence's realization
* `cpt-cf-onnx-runner-phase-model-state` — the state machine and reason codes
* `cpt-cf-onnx-runner-phase-degraded-startup` — the permissive default and its escape hatch
* `cpt-cf-onnx-runner-principle-no-payload-in-telemetry` — satisfied by construction, via a
  fieldless reason enum
* `cpt-cf-onnx-runner-principle-fail-at-load` — upheld: loading is still eager and still
  verifies at startup; what changed is the blast radius of a failure, not its timing
* `cpt-cf-onnx-runner-contract-client-v1` — extended with `ModelUnavailable` and model state
* `cpt-genaigw-fr-resilient-service` — the upstream requirement this enables a consumer to
  satisfy, and does not satisfy on its own
