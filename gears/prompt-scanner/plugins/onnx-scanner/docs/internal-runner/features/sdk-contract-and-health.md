Created:  2026-08-26 by Constructor Tech
Updated:  2026-08-26 by Constructor Tech

# Feature: SDK Contract and Health

<!-- toc -->

- [1. Feature Context](#1-feature-context)
- [2. Current State](#2-current-state)
- [3. Scope](#3-scope)
- [4. Phases](#4-phases)
  - [Phase 1: Serializable SDK Types](#phase-1-serializable-sdk-types)
  - [Phase 2: Platform Healthcheck](#phase-2-platform-healthcheck)
  - [Phase 3: The SecurityContext Position](#phase-3-the-securitycontext-position)
- [5. Feature Acceptance Criteria](#5-feature-acceptance-criteria)
- [6. Non-Functional Considerations](#6-non-functional-considerations)
- [7. Outcome](#7-outcome)

<!-- /toc -->

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-featstatus-sdk-contract`

## 1. Feature Context

- [x] `p1` - `cpt-cf-onnx-runner-feature-sdk-contract`

### Overview

Finish the public contract: serializable SDK types, operator-visible readiness through the
platform healthcheck registry, and a decided position on `SecurityContext`.

**Closes**: [DESIGN.md](../../DESIGN.md) §5 items 9, 10, 12.

### Purpose

Three loose ends in the contract, none of which needs an HTTP endpoint.

*Serialization.* The SDK's types derive `Debug, Clone, PartialEq` and nothing else, so no
out-of-process transport is possible. The repo supports out-of-process gears as a
first-class mode (`docs/toolkit_unified_system/09_oop_grpc_sdk_pattern.md`); this gear is
structurally excluded from it. For a component whose selling point is reusable ONNX
execution, that matters — an inference gear is exactly the kind of thing someone will want
to isolate or scale separately. Adding the derives now costs almost nothing and removes the
blocker; building the gRPC sidecar is separate, later, and only when a consumer asks.

*Readiness.* Operators have no way to learn that a model failed to load. The platform
already solves this: implement `Healthcheck`, return it from `RestApiCapability::healthcheck`,
and API Gateway aggregates per-component status into `/health` and `/readyz`
(`libs/toolkit/src/healthcheck.rs`). That is the whole job — no endpoint of our own, no
DTOs, no schema.

*Authorization.* Every trait method takes a `SecurityContext` and binds it to `_ctx`. That
is the worst of both options: it looks enforced and is not. Decide, and write it down.

**Explicitly not in this feature: any HTTP API for this gear.** See
`cpt-cf-onnx-runner-constraint-no-http-surface` and [DESIGN.md](../../DESIGN.md) §3.5 for why
that is a standing non-goal rather than deferred work.

### References

- **Gear design**: [DESIGN.md](../../DESIGN.md) §3.2, §3.3, §3.5, §5
- **Platform health**: `libs/toolkit/src/healthcheck.rs` — `Healthcheck`,
  `HealthcheckResult`, `RestHealthcheckRegistry`; example usage in the module docs
- **Repo standards**: `docs/toolkit_unified_system/09_oop_grpc_sdk_pattern.md`,
  `02_gear_layout_and_sdk_pattern.md`
- **Depends on**: [model-lifecycle](./model-lifecycle.md) — per-model state is what the
  healthcheck reports

## 2. Current State

- `onnx-runner-sdk`'s `NamedTensor`, `TensorData`, `ModelInfo`, `TokenEncoding` derive
  `Debug, Clone, PartialEq` only. No `Serialize`/`Deserialize`.
- The gear implements no `Healthcheck`, so platform `/readyz` and `/health` report nothing
  about model availability. A gear serving zero loaded models looks identical to a healthy
  one.
- `OnnxRunnerError` has no severity or classification beyond its variants — fine for an
  in-process contract, but consumers currently cannot distinguish retryable from terminal.
- Every trait method takes `ctx: &SecurityContext` and ignores it.
- The gear crate's `lib.rs` lacks `#![forbid(unsafe_code)]`, which the SDK crate has.

## 3. Scope

**In scope:**

- serde on SDK types, with a considered wire representation for tensor data.
- A platform `Healthcheck` implementation reporting per-model readiness.
- An explicit, documented position on `SecurityContext` and authorization.
- Retryable-vs-terminal classification on the error type.
- `#![forbid(unsafe_code)]` on the gear crate.

**Out of scope:**

- **Any HTTP surface for this gear** — a standing non-goal, not deferred work.
- gRPC/OoP wiring. This feature removes the serde blocker; building the sidecar waits for a
  real consumer.
- Per-tenant quota enforcement, which needs a policy source this gear does not have.

## 4. Phases

### Phase 1: Serializable SDK Types

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-phase-sdk-serde`

**Requirements**

1. `Serialize`/`Deserialize` on `ModelInfo`, `TokenEncoding`, `NamedTensor`, `TensorData`,
   and the state/reason types from [model-lifecycle](./model-lifecycle.md).
2. A deliberate wire representation for `TensorData`: an externally-tagged enum keyed by
   dtype, with numeric arrays. Documented, including the size cost for large tensors and the
   recommendation to use the in-process client for bulk inference.
3. Round-trip tests for every variant, including empty data and a large-shape case.
4. The SDK gains no framework dependency — no HTTP types, no `utoipa`, no gRPC codegen.
   Serialization is a property of the types, not a transport commitment.

**Acceptance criteria**

- [x] Every SDK type round-trips through JSON with equality.
      *(`serde_tests` in `models.rs`, all seven types, every dtype, empty and large shapes.)*
- [x] The SDK crate's dependency list grows by at most `serde`. *(Exactly `serde`;
      `serde_json` is a dev-dependency for the round-trip tests.)*
- [x] The wire format for `TensorData` is documented in its doc comment, with its size
      characteristics. *(And pinned by a test, so a rename is a visible break.)*

### Phase 2: Platform Healthcheck

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-phase-healthcheck`

**Requirements**

1. A `Healthcheck` implementation, returned from `RestApiCapability::healthcheck`, named
   distinctly (e.g. `onnx-runner-models`).
2. Its result reflects per-model state from [model-lifecycle](./model-lifecycle.md):
   healthy when every configured model is `Ready`; degraded/unhealthy when any is
   `Unavailable`, naming which and its reason classification.
3. The check is cheap and non-blocking — it reads recorded state, never runs an inference.
4. Zero loaded models is reported unhealthy, not healthy-by-vacuity.
5. The check body carries no payload content, no absolute paths, and no tensor data
   (`cpt-cf-onnx-runner-principle-no-payload-in-telemetry`).
6. `register_rest` remains a no-op returning the router unchanged — implementing
   `RestApiCapability` for its `healthcheck` hook is **not** an HTTP surface, and a comment
   at the impl says so, so nobody later reads it as an invitation to add routes.

**Acceptance criteria**

- [x] A gear with one unavailable model reports a degraded healthcheck naming that model.
- [x] A gear with all models ready reports healthy.
- [x] A gear with zero models loaded reports unhealthy. *(Both senses: none configured, and
      configured-but-none-loaded, which get distinct codes.)*
- [x] The check completes without touching a session; asserted by timing or by construction.
      *(By construction, asserted through the metrics port: five probes record no inference.)*
- [x] `register_rest` adds no routes, and a test asserts the gear registers none.
      *(Four paths driven through the returned router, including the
      `/onnx-runner/v1/models` endpoint the old docs falsely claimed; all 404.)*

### Phase 3: The SecurityContext Position

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-phase-authz-position`

**Requirements**

1. **The decision, taken and recorded.** Two defensible options:
   - *Trusted in-process compute primitive*: no authorization here; access control belongs
     to the consumer that owns the tenant-facing operation. The `ctx` parameter is then
     either removed or documented as propagation-only, for tracing.
   - *Enforcing gear*: authorization on model access, enabling per-tenant model
     restrictions later.

   Recommended: the first, keeping the parameter for trace propagation with its
   non-enforcement documented at the trait. The gear owns no tenant-scoped resource, and a
   silently-ignored `ctx` is worse than an honestly-documented one. **If the recommendation
   is rejected, enforcement is a larger piece of work needing its own estimate — raise it
   before starting, not during.**
2. Whichever is chosen, no method silently ignores a parameter that looks enforced.
3. Removing `ctx` would be a breaking change: that is `OnnxRunnerClientV1V2`, not an edit
   to V1 (`cpt-cf-onnx-runner-contract-client-v1`).
4. Error variants are classified retryable (timeout, overload) versus terminal (unknown
   model, invalid input, contract mismatch), so consumers can act on them without matching
   on strings.
5. `#![forbid(unsafe_code)]` added to the gear crate's `lib.rs`.

**Acceptance criteria**

- [x] The `SecurityContext` position is documented on the trait and in an ADR.
      → [ADR 0011](../../ADR/0011-cpt-cf-onnx-runner-adr-security-context-position.md)
- [x] Every error variant carries a retryable/terminal classification, asserted in a test.
      *(`OnnxRunnerError::is_retryable`, which predated this phase.)*
- [x] The gear crate forbids unsafe code. *(Both crates; also enforced workspace-wide.)*

## 5. Feature Acceptance Criteria

- [x] SDK types are serializable, unblocking a future out-of-process transport without
      committing to one.
- [x] Model availability is visible to operators through platform `/health` and `/readyz`.
- [x] The authorization posture is a written decision rather than an omission.
- [x] Consumers can distinguish retryable from terminal failures.
- [x] The gear still exposes no HTTP routes of its own, and a test enforces that.
- [x] ADRs written: the no-HTTP-surface position ([0003](../../ADR/0003-cpt-cf-onnx-runner-adr-no-http-surface.md),
      landed early in Stage 0), and the `SecurityContext` position
      ([0011](../../ADR/0011-cpt-cf-onnx-runner-adr-security-context-position.md)).

## 6. Non-Functional Considerations

**Security.** Adding no HTTP surface means adding no new attack surface to a gear that loads
400 MB artifacts and runs native code. The healthcheck body is the only new
externally-visible output; it must name models and reasons, never paths or payloads.

**Compatibility.** serde derives are source-compatible. Removing `ctx` would not be — see
Phase 3 requirement 3.

**Documentation debt this closes.** Earlier revisions of this gear's docs claimed a
`GET /onnx-runner/v1/models` endpoint had shipped; it never existed, and it is now recorded
as a deliberate non-goal rather than missing work. That framing is the point: "deferred"
invites someone to build it later without a consumer.

## 7. Outcome

**The wire format.** `TensorData` is an externally-tagged enum keyed by the dtype's lowercase
name — `{"f32": [0.1, 0.2]}` — with the shape kept on `NamedTensor` rather than inside the
data, so a shape/data disagreement stays one validation at the boundary. Its doc comment
carries the size cost (a 512×28 classifier output is ~200 KB of JSON against 57 KB of bytes)
and says plainly that these derives unblock an out-of-process transport rather than recommend
one for bulk inference. A test pins the exact JSON, so a rename is a visible break rather than
a silent one.

**One spelling for three audiences.** `ModelFailureReason` serializes to the same `snake_case`
string `as_code()` returns, so a reason on the wire, in a log line, and in the healthcheck body
are spelled identically; a test asserts the equality across all six variants. That was not in
the spec — it emerged from doing Phases 1 and 2 together, and it is the reason the healthcheck
body needed no vocabulary of its own.

**`deny_unknown_fields` deliberately off.** Strictness is right for operator YAML, where it
catches typos; it is wrong for a wire type, where it breaks a newer sender talking to an older
receiver. Noted because the gear's `ModelConfig` does the opposite, and the inconsistency is
intentional.

**The healthcheck's shape.** One composite check, `onnx-runner-models`, over a `model_snapshot`
extracted from `list_models` — so operators and consumers read one projection rather than two
that can disagree. Four verdicts: no models configured → unhealthy `no_models_configured`; none
ready → unhealthy `no_models_ready`; some ready → **degraded** `models_unavailable`; all ready →
healthy. Degraded rather than unhealthy for a partial outage is the load-bearing choice: it
keeps the pod in rotation, which is correct because the loaded models still serve and a consumer
asking for a broken one gets a typed `ModelUnavailable` it must map to failed coverage. `Loading`
counts as not-ready but degrades rather than fails.

**`/health` is unauthenticated in every serving mode**, per the toolkit's own module docs, and
the registry's sanitization is not something to lean on. The body therefore carries only
operator-chosen model ids and stable reason codes. One test pins the whole message string and
asserts no `/` and no `.onnx` appear.

**The authorization decision: trusted in-process compute primitive.** `ctx` stays for trace
propagation, and the trait now states that nothing enforces it, why, and what would have to
change. The reasoning is that the gear owns no tenant-scoped resource — models are
operator-configured and addressed by id, a caller can never supply a path — so there is nothing
for an authorization decision to be about, and no policy source to consult if there were. The
sentence that does the work is at the trait: *do not treat a successful call as evidence that
the subject in `ctx` was permitted anything.* Documenting beat removing on cost: removal is
`OnnxRunnerClientV1V2`, a breaking change bought for clarity a doc comment gives for free, and
it would forfeit trace propagation. Rationale in
[ADR 0011](../../ADR/0011-cpt-cf-onnx-runner-adr-security-context-position.md).

**Two requirements were already met** when the phase started: `is_retryable()` existed, and both
crates forbid unsafe code. Phase 3 was therefore the decision and its documentation, nothing
more.

**Found on the way.** The gear declared no capabilities at all, so `capabilities = [rest]` had to
be added for the `healthcheck` hook to be called — the exact reading the constraint warns
against. `register_rest` returns the router untouched, the impl block says why in a comment
addressed to whoever arrives wanting to add a route, and `registers_no_rest_routes` fails if that
stops being true.
