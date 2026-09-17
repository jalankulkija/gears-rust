---
status: accepted
date: 2026-08-26
decision-makers: onnx-runner gear maintainers
---

# Expose no HTTP API; reach the gear in-process and report readiness through the platform healthcheck

**ID**: `cpt-cf-onnx-runner-adr-no-http-surface`

<!-- toc -->

- [Context and Problem Statement](#context-and-problem-statement)
- [Decision Drivers](#decision-drivers)
- [Considered Options](#considered-options)
- [Decision Outcome](#decision-outcome)
  - [Consequences](#consequences)
  - [Confirmation](#confirmation)
- [Pros and Cons of the Options](#pros-and-cons-of-the-options)
  - [No HTTP surface; platform healthcheck for readiness](#no-http-surface-platform-healthcheck-for-readiness)
  - [REST introspection endpoints](#rest-introspection-endpoints)
  - [REST including inference](#rest-including-inference)
  - [gRPC / out-of-process mode now](#grpc--out-of-process-mode-now)
- [More Information](#more-information)
- [Traceability](#traceability)

<!-- /toc -->

## Context and Problem Statement

The gear's documentation claimed a `GET /onnx-runner/v1/models` endpoint had shipped, citing an
`api/rest/` directory as evidence. Neither exists: there is no `api/` directory and the gear
registers no REST capability. The gear's own module comment described the endpoint as
"deferred", framing it as pending work.

That left an unresolved question posing as a resolved one. Should this gear have an HTTP API at
all — and if the answer is no, is that "not yet" or "not ever"? A capability described as
deferred invites someone to build it later without a consumer, which is how the false claim
arose in the first place.

## Decision Drivers

* No consumer needs it. The only consumer, `onnx-scanner`, resolves the client through
  `ClientHub`. No second consumer exists, in this program or elsewhere.
* The gear owns no tenant-facing resource. There is no model catalog to administer, no
  per-tenant state, nothing an endpoint would meaningfully address.
* The operator need — "did my models load?" — is already solved by platform machinery we were
  not using.
* An HTTP surface on a component that loads 441 MB third-party artifacts and runs native
  inference code is attack surface that must be justified by a use case, not added by default.
* Tensors over JSON are a poor transport: large, lossy about dtypes, and slower than the
  in-process call they would replace. If out-of-process inference is ever needed, the repo
  already has a better answer.
* The gear's category in this repo is "internal service", and that category consistently has no
  REST surface.

## Considered Options

* No HTTP surface; readiness through the platform `Healthcheck` registry
* REST introspection endpoints (`GET /models`, `GET /models/{id}`)
* REST including an inference endpoint
* gRPC / out-of-process mode now

## Decision Outcome

Chosen option: **no HTTP surface; readiness through the platform `Healthcheck` registry**,
because there is no consumer for an endpoint, the operator need is fully met by
`libs/toolkit/src/healthcheck.rs` at a fraction of the cost, and the gear belongs to the class
of in-process gears in this repo that deliberately have no REST surface.

This is recorded as a **standing non-goal, not deferred work**. Reopening it requires a named
consumer and a stated need, at which point gRPC — not JSON over HTTP — is the starting point.

### Consequences

* The gear registers no routes. If it implements `RestApiCapability`, it does so solely for the
  `healthcheck()` hook, with `register_rest` returning the router unchanged and a comment saying
  why — so the trait's presence is not later read as an invitation.
* Readiness is reported by implementing `Healthcheck` and returning it from
  `RestApiCapability::healthcheck`. API Gateway aggregates per-component status into `/healthz`,
  `/readyz`, and `/health`, so model availability becomes visible to operators without this gear
  serving anything itself. Specified in
  the SDK contract and health work Phase 2.
* Per-model readiness detail stays on the in-process client contract (`list_models`, model
  state), which is where the consumer needs it anyway to report degraded coverage.
* SDK types still get `Serialize`/`Deserialize`, and this decision does not justify that work —
  serde is justified independently, as the prerequisite for a future out-of-process transport.
  The two must not be conflated: adding serde is not a step toward REST.
* No RFC9457 problem-detail mapping is needed, because no error crosses an HTTP boundary. Errors
  instead gain a retryable-versus-terminal classification, which is what an in-process consumer
  actually needs.
* If an out-of-process consumer appears, the path is the OoP/gRPC SDK pattern
  (`docs/toolkit_unified_system/09_oop_grpc_sdk_pattern.md`) — a `RuntimeKind::Oop` gear with a
  gRPC client in the SDK crate. That is a new decision with its own ADR, and this one does not
  pre-approve it.
* The estimate for the gear's contract work dropped from 4 days to 2, and the release bar item
  changed from "REST introspection with OpenAPI" to "readiness visible via platform health".
* Every mention of a REST API for this gear was removed from its docs, the program docs, and the
  source comments, and replaced by `cpt-cf-onnx-runner-constraint-no-http-surface`. Removing the
  claim without recording the decision would have let it return.

### Confirmation

* A test asserts the gear registers no routes — the mechanical guard against re-introduction.
* Healthcheck tests: all models ready reports healthy; one unavailable model reports degraded and
  names it; zero loaded models reports unhealthy rather than healthy-by-vacuity.
* Code review rule: any future `api/` directory in this gear requires an ADR superseding this
  one, with a named consumer.

## Pros and Cons of the Options

### No HTTP surface; platform healthcheck for readiness

Chosen. In-process via `ClientHub`; readiness contributed to the platform's aggregated health.

* Good, because it matches the gear's actual category. Nine non-plugin gears in this repo have
  no `api/rest`: `authn-resolver`, `authz-resolver`, `tenant-resolver`, `rate-provider`,
  `cluster`, `grpc-hub`, `api-gateway`, `forward-proxy`, and this one. REST correlates with
  owning tenant-facing resources — `file-parser`, `chat-engine`, `prompt-scanner`,
  `resource-group` — which this gear does not
* Good, because the operator need is met in roughly twenty lines instead of a DTO layer, handler
  layer, route registration, OpenAPI schemas, and their tests
* Good, because it adds no attack surface to a gear running native code over third-party model
  artifacts
* Good, because readiness lands in the same aggregated endpoint operators already watch, rather
  than a gear-specific path they would have to learn
* Neutral, because it forecloses nothing: gRPC/OoP remains available when a consumer justifies it
* Bad, because a human debugging by hand cannot `curl` the gear directly for its model list; they
  read the aggregated `/health` body or the startup logs instead
* Bad, because a future out-of-process consumer will need transport work that a REST surface
  would have partially provided — though not in a form worth keeping

### REST introspection endpoints

`GET /models`, `GET /models/{model_id}` with OpenAPI, per the original design sketch.

* Good, because model state becomes directly inspectable with a browser or `curl`
* Good, because it follows the canonical gear layout, so it looks conventional
* Neutral, because it is cheap in isolation — roughly a day and a half
* Bad, because no consumer asked for it; it would exist to satisfy a documentation claim rather
  than a need
* Bad, because it duplicates readiness reporting that the platform healthcheck already
  aggregates, creating two sources of the same truth
* Bad, because an endpoint invites growth: once `GET /models` exists, "just add a POST for
  inference" is a small step with none of the analysis this decision performs

### REST including inference

Add `POST /models/{id}/run` accepting JSON tensors.

* Good, because it would make the gear usable from any language over HTTP
* Bad, because JSON tensors are large, slow, and imprecise about dtypes — the exact class of
  error `output_dtypes` strictness exists to eliminate
* Bad, because it exposes the most resource-intensive operation to the widest audience, and the
  gear's bounds work would have to be hardened for untrusted callers first
* Bad, because gRPC with a typed schema is strictly better for the same goal
* Bad, because no consumer exists

### gRPC / out-of-process mode now

Implement `RuntimeKind::Oop` with a gRPC server and SDK client.

* Good, because it is the right eventual answer for scaling or isolating inference
* Good, because it would let the inference gear be sized independently of its consumers
* Neutral, because serde on the SDK types — which this cycle does anyway — is its prerequisite
* Bad, because it is speculative: no consumer has asked to run this gear out of process
* Bad, because it adds a process lifecycle, transport configuration, and failure modes to a gear
  that has not yet fixed blocking inference on the async runtime — wrong order of work

## More Information

The false claim is instructive about documentation drift: the endpoint appeared in a design
sketch's directory listing, was then described as shipped in a decomposition table, and was
required by a feature file's acceptance criteria — while the source comment said it had been
deliberately skipped. Three documents inherited an intention from one sketch, and the code was
the only honest party.

`libs/toolkit/src/healthcheck.rs` was the missing piece in the earlier reasoning. Its existence
turns "operators need an endpoint" into "operators need us to implement one trait", which is what
collapsed the justification for the whole feature.

## Traceability

- **DESIGN**: [DESIGN.md](../DESIGN.md) §1.3, §3.5

This decision directly addresses the following requirements or design elements:

* `cpt-cf-onnx-runner-constraint-no-http-surface` — this ADR is that constraint's rationale
* `cpt-cf-onnx-runner-component-rest-api` — the element this decision **removes**; it existed
  only in documentation and is now a recorded non-goal
* `cpt-cf-onnx-runner-contract-client-v1` — remains the sole entry point to the gear
* `cpt-cf-onnx-runner-feature-sdk-contract` — the feature this decision rescoped, from a REST
  surface to serde plus a platform healthcheck
* `cpt-cf-onnx-runner-seq-degraded-load` — the healthcheck is how a degraded load becomes visible
  outside the process
