---
status: accepted
date: 2026-08-31
decision-makers: onnx-runner gear maintainers
---

# Hard-code the CPU execution provider; offer no `execution_provider` knob

**ID**: `cpt-cf-onnx-runner-adr-cpu-only-execution`

<!-- toc -->

- [Context and Problem Statement](#context-and-problem-statement)
- [Decision Drivers](#decision-drivers)
- [Considered Options](#considered-options)
- [Decision Outcome](#decision-outcome)
  - [Consequences](#consequences)
  - [Confirmation](#confirmation)
- [Pros and Cons of the Options](#pros-and-cons-of-the-options)
  - [CPU only, not configurable](#cpu-only-not-configurable)
  - [A configurable execution provider](#a-configurable-execution-provider)
  - [Automatic provider selection](#automatic-provider-selection)
- [More Information](#more-information)
- [Traceability](#traceability)

<!-- /toc -->

## Context and Problem Statement

ONNX Runtime supports execution providers other than CPU — CoreML, CUDA, DirectML and
others — and `ort` exposes them. A gear whose whole purpose is running ONNX models is the
obvious place to make that choice configurable, and "which accelerator?" is the first
question anyone asks about an inference component.

Offering the knob is close to free to implement and expensive to be wrong about: every
provider is a distinct code path through ONNX Runtime's graph partitioner, and a path
nobody exercises is a path that fails in front of a customer.

## Decision Drivers

* The workload is small PII-detection models over short prompts, in a proxy's request
  path, many callers at once.
* A configuration option implies a support commitment. A knob whose non-default values are
  untested is worse than no knob.
* Accelerator claims should be measured on the workload, not inherited from benchmarks of
  a different one.

## Considered Options

* **CPU only, not configurable** — hard-code the provider.
* **A configurable execution provider** — expose it in `ModelConfig`.
* **Automatic provider selection** — probe at startup and use the best available.

## Decision Outcome

Chosen: **CPU only, not configurable.**

Two measured facts drove it.

**CoreML was a regression, not an improvement.** On Apple hardware it was measured slower
than CPU for these models. The cause is structural rather than incidental: the models use
dynamic shapes, and their quantized exports contain cast/dequantize patterns CoreML cannot
fuse, so the graph partitions into many small CoreML subgraphs with CPU hops between them.
Nothing about that improves with a newer model of the same shape.

**The workload is memory-bandwidth-bound, not compute-bound.** The best arrangement in the
sizing sweep reaches roughly a *third* of what linear scaling from single-thread
performance would predict — see
[ADR 0009](./0009-cpt-cf-onnx-runner-adr-replica-pool-sizing.md). Additional compute is not
the binding constraint, so an accelerator is answering a question this workload does not
ask.

The provider is therefore hard-coded, deliberately, rather than offered as a knob whose
non-CPU paths are untested.

### Consequences

* Good, because there is no configuration surface whose interesting values are unverified.
* Good, because `ModelConfig` stays about the *model* — file, checksum, expected tensors —
  rather than about the host.
* Good, because it keeps the gear's behaviour identical on a developer laptop and a
  server, which matters for reproducing a support issue.
* Bad, because a deployment with a GPU cannot use it without a code change. That is the
  intended trade: the change should come with the measurement that justifies it.
* Neutral, because nothing in the design precludes adding providers later. Adding one is a
  new `SessionBuilder` path plus the benchmarks to defend it; no consumer contract changes.

### Confirmation

There is nothing to assert in a test, which is the point: `#[serde(deny_unknown_fields)]`
on `ModelConfig` means an `execution_provider` key in YAML fails gear init loudly rather
than being silently ignored, so a configuration written against a different expectation
cannot quietly do nothing.

## Pros and Cons of the Options

### CPU only, not configurable

* Good, because every shipped path is exercised on every developer machine and in CI.
* Good, because it matches the measurement: CoreML was slower and the workload is
  bandwidth-bound.
* Bad, because it forecloses accelerated deployments until someone does the work.

### A configurable execution provider

* Good, because it looks complete, and answers the question everyone asks.
* Bad, because the non-CPU values would ship untested, and the one measured alternative
  was a regression.
* Bad, because provider availability is a property of the host, so a config file becomes
  non-portable in a way that fails at run time rather than at parse time.

### Automatic provider selection

* Good, because it needs no operator decision.
* Bad, because it makes throughput depend on what happens to be installed on the host —
  the same config, the same model, different performance, with nothing in the
  configuration explaining why.
* Bad, because on the one platform measured it would have selected the slower path.

## More Information

`intra_threads` remains configurable, and is the knob that actually moves this workload;
[ADR 0009](./0009-cpt-cf-onnx-runner-adr-replica-pool-sizing.md) covers how it interacts
with replica count and why the product of the two is the real budget.

## Traceability

- **DESIGN**: [DESIGN.md](../DESIGN.md) §4 (decision 3)

This decision directly addresses the following requirements or design elements:

* `cpt-cf-onnx-runner-decision-cpu-execution` — this ADR is that decision's rationale
* `cpt-cf-onnx-runner-constraint-no-untested-config` — a configuration option implies a
  tested path; this is the clearest application of that constraint
* `cpt-cf-onnx-runner-nfr-throughput` — bounds what the gear promises, and on what hardware
