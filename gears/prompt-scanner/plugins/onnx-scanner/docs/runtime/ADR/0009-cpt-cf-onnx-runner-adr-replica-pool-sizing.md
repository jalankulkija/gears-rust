---
status: accepted
date: 2026-08-31
decision-makers: onnx-runner gear maintainers
---

# Size the replica pool by replica count, not by intra-op threads

**ID**: `cpt-cf-onnx-runner-adr-replica-pool-sizing`

<!-- toc -->

- [Context and Problem Statement](#context-and-problem-statement)
- [Decision Drivers](#decision-drivers)
- [Considered Options](#considered-options)
- [Decision Outcome](#decision-outcome)
  - [Consequences](#consequences)
  - [Confirmation](#confirmation)
- [Pros and Cons of the Options](#pros-and-cons-of-the-options)
  - [Many thin replicas](#many-thin-replicas)
  - [One wide session](#one-wide-session)
  - [The extreme of the sweep](#the-extreme-of-the-sweep)
- [More Information](#more-information)
- [Traceability](#traceability)

<!-- /toc -->

## Context and Problem Statement

[ADR 0004](./0004-cpt-cf-onnx-runner-adr-blocking-pool-offload.md) settled *that* there is
a pool of sessions rather than one shared session: `ort` takes `&mut self` on
`Session::run` because ONNX Runtime's `Run` is not thread-safe, so sharing one session
concurrently is undefined behaviour rather than a trade-off.

It did not settle *how many*. A host has a fixed number of cores, and two knobs spend
them: the number of session replicas, and the intra-op thread count each replica is given.
Their product is roughly the thread budget. Nothing in the ONNX Runtime documentation says
which way to divide it, and the two knobs pull in opposite directions — more replicas means
more concurrent calls in flight, more threads per replica means each call finishes sooner.

Guessing here is expensive: the wrong split is invisible in correctness tests and shows up
only as throughput a consumer never gets.

## Decision Drivers

* The request path is concurrent: many callers, each with one modest prompt. Aggregate
  throughput matters more than single-call latency.
* Memory is the cost. Each replica is a full copy of the model weights *plus* its own
  intra-op thread pool.
* The default must be defensible without a benchmark in hand, because most operators will
  never run one.
* Server hardware is not laptop hardware, and a default tuned on the wrong one is worse
  than no default.

## Considered Options

* **Many thin replicas** — maximise replica count, give each a small thread count.
* **One wide session** — a single replica with all available intra-op threads.
* **The extreme of the sweep** — as many replicas as the thread budget allows.

## Decision Outcome

Chosen: **many thin replicas**, with the shipped default at four replicas.

At a fixed budget of 48 threads on a 48-core, two-socket host, running `gliner2-pii`:

| Arrangement | 128 tok, 64 callers | 512 tok, 64 callers | Peak RSS |
|---|---|---|---|
| 1 x 48 | 10.2/s | 2.1/s | 1.2 GB |
| 2 x 24 | 18.9/s | 3.2/s | 2.0 GB |
| 6 x 8 | 32.1/s | 5.1/s | 5.3 GB |
| 12 x 4 | 35.1/s | **5.9/s** | 10.1 GB |
| 24 x 2 | **39.4/s** | 5.1/s | 19.1 GB |

Many thin replicas sustain roughly **four times** the concurrent throughput of one wide
session. The mechanism is not subtle: a single session cannot spread 48 intra-op threads
across two sockets, so most of the machine is unreachable from one replica however many
threads it is given.

The same sweep on a four-thread laptop moves the numbers from 7.9/s to 8.6/s — the same
direction, an order of magnitude less of it. Ratios measured on development hardware do
not transfer, which is why the table above is the server one.

### Consequences

* Good, because the default arrangement is chosen from measurement rather than from the
  shape of the API.
* Good, because `replicas` is per-model configuration, so an operator who has measured
  their own host can override it, and one who has not gets a sane default.
* Bad, because the operator pays in memory, and the bill is steep: 24 replicas buy 23%
  more short-prompt throughput than 6 for **3.6x** the resident memory.
* Bad, because the optimum depends on prompt length. 24 replicas win at 128 tokens and
  *lose* to 12 at 512 — there is no single best number, only a defensible default.
* Neutral, because two known reductions are unused. `PrepackedWeights` would share weight
  copies across replicas, and an environment-level shared thread pool would share the
  thread pools; `ort` requires that a library never configure the ONNX Runtime environment,
  so the latter is an application-level decision this gear cannot make.

### Confirmation

`replicas` is validated at gear init and a zero value is rejected rather than silently
treated as one — see `infra::ort_session`'s tests. The throughput claim itself is not a
unit test: it is a measurement, reproduced by the sweep in this ADR, and it would be
dishonest to assert it in CI on hardware that cannot show it.

## Pros and Cons of the Options

### Many thin replicas

* Good, because it reaches roughly 4x the throughput of one wide session on the reference
  host.
* Good, because concurrent callers queue against replicas rather than against one mutex.
* Bad, because memory scales linearly with replica count.

### One wide session

* Good, because it is the cheapest possible memory footprint (1.2 GB in the sweep).
* Good, because it needs no pool, no acquisition and no sizing decision.
* Bad, because it reaches 10.2/s where the pool reaches 39.4/s.
* Bad, because intra-op threading cannot cross a socket boundary, so the extra threads a
  wide session is given are partly wasted on a multi-socket host.

### The extreme of the sweep

* Good, because it is the best short-prompt number measured (39.4/s at 24 x 2).
* Bad, because it costs 19.1 GB resident for that number.
* Bad, because it is *slower* than 12 x 4 on long prompts, so the extreme is not the
  optimum — it only looks like it if one prompt length is measured.

## More Information

Detection quality for the same models, which is the other half of any model decision, is
in [benchmarks/QUALITY-RESULTS.md](../benchmarks/QUALITY-RESULTS.md). Neither number means
much without the other: a model that is faster and misses PII is worse than useless.

## Traceability

- **DESIGN**: [DESIGN.md](../DESIGN.md) §2.2, §4 (decision 2)

This decision directly addresses the following requirements or design elements:

* `cpt-cf-onnx-runner-decision-replica-pool` — this ADR is that decision's measured basis
* `cpt-cf-onnx-runner-constraint-session-not-thread-safe` — the reason a pool exists at all
  is established in ADR 0004; this ADR only sizes it
* `cpt-cf-onnx-runner-nfr-throughput` — the sweep is the evidence for what the gear can
  sustain, and on what hardware
