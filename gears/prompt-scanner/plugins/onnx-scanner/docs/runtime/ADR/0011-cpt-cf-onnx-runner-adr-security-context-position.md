---
status: accepted
date: 2026-09-08
decision-makers: onnx-runner gear maintainers
---

# Carry `SecurityContext` for trace propagation and enforce no authorization, stated at the trait

**ID**: `cpt-cf-onnx-runner-adr-security-context-position`

<!-- toc -->

- [Context and Problem Statement](#context-and-problem-statement)
- [Decision Drivers](#decision-drivers)
- [Considered Options](#considered-options)
- [Decision Outcome](#decision-outcome)
  - [Consequences](#consequences)
  - [Confirmation](#confirmation)
- [Pros and Cons of the Options](#pros-and-cons-of-the-options)
  - [Trusted in-process primitive, non-enforcement documented](#trusted-in-process-primitive-non-enforcement-documented)
  - [Enforcing gear](#enforcing-gear)
  - [Remove the parameter](#remove-the-parameter)
  - [Leave it as it was](#leave-it-as-it-was)
- [More Information](#more-information)
- [Traceability](#traceability)

<!-- /toc -->

## Context and Problem Statement

Every method on `OnnxRunnerClientV1` takes `ctx: &SecurityContext`. Every implementation binds
it to `_ctx` and does nothing with it. The trait said nothing about this either way.

That is the worst of the available arrangements. A reader encountering the signature reasonably
concludes the gear authorizes something, because that is what a `SecurityContext` parameter
normally means in this repo. A reader encountering the `_ctx` binding concludes enforcement was
forgotten. Both readings are wrong, and neither is correctable from the code, because the code
does not say which it is.

The question is not primarily "should this gear authorize?" — it is "what is the posture, and
where is it written down?" An unstated posture becomes an assumed one, and the assumption that
gets made is the dangerous direction: that a call which succeeded was a call that was permitted.

## Decision Drivers

* The gear owns no tenant-scoped resource. Models are operator-configured at startup and
  addressed by id; there is no per-tenant model, no per-tenant state, and nothing an
  authorization decision could be *about*.
* A caller can never supply a filesystem path, URL, or model bytes
  (`cpt-cf-onnx-runner-constraint-no-caller-paths`). The attack surface that authorization would
  usually guard — reaching a resource you should not reach — was removed at the contract level
  rather than at the policy level.
* There is no policy source available here. Enforcing anything meaningful would require a
  per-tenant model-permission model that does not exist in this program, plus somewhere to
  administer it.
* The gear is reached in-process through `ClientHub`, never over a network
  (`cpt-cf-onnx-runner-constraint-no-http-surface`). Its callers are other gears in the same
  process, already inside the trust boundary.
* Sibling gears in the same category — `authn-resolver`, `authz-resolver`, `tenant-resolver`,
  `cluster` — take the same position: in-process compute primitives that do not re-authorize
  their callers.
* Trace propagation is a real, current use for the parameter, and it is the only one.
* Removing the parameter is not free: the trait is versioned by name, so it would be
  `OnnxRunnerClientV1V2` rather than an edit
  (`cpt-cf-onnx-runner-contract-client-v1`).

## Considered Options

* Trusted in-process compute primitive; keep `ctx`, document non-enforcement at the trait
* Enforcing gear: authorize model access per call
* Remove `ctx` from the contract entirely
* Leave it undocumented

## Decision Outcome

Chosen option: **trusted in-process compute primitive**. `ctx` stays on all four methods for
trace propagation and for the option of enforcing later, and the trait states in its own
documentation that nothing enforces it, why, and what would have to change for that to stop
being true.

The load-bearing sentence, at the trait: *do not treat a successful call as evidence that the
subject in `ctx` was permitted anything.*

### Consequences

* `OnnxRunnerClientV1` gains an `# Authorization: this gear enforces none` section covering the
  posture, the reasoning, what it means for a caller, and the two ways it could change. The
  implementation in `gear.rs` carries a shorter note at the impl block, where a reader meets the
  `_ctx` bindings, pointing at the trait and this ADR — so the unused parameter is not later
  "fixed" by someone adding a check with no policy behind it.
* Access control for anything this gear computes remains the consumer's responsibility, at the
  tenant-facing operation that called it. For `onnx-scanner` that is the scan; the gear is an
  implementation detail of it.
* The signature does not change, so nothing breaks and no version bump is needed. This is the
  main practical reason to prefer documenting over removing: the honest-by-deletion option costs
  a contract version and forfeits trace propagation, and buys clarity that a doc comment
  provides for nothing.
* If per-tenant model restrictions are ever required, that is a feature with its own design and
  estimate — it needs a policy source, an administration surface, and tests — not a check
  inserted into `run()`. This ADR does not pre-approve it, and reopening the question should
  supersede this record rather than amend it.
* The `SecurityContext` is available for spans should richer trace attribution be wanted later.
  Note the constraint that governs what may be put there: subject and tenant identifiers are not
  payload, but tensor contents and tokenized text are, and those must never reach telemetry
  (`cpt-cf-onnx-runner-principle-no-payload-in-telemetry`).
* Error variants already distinguish retryable from terminal (`OnnxRunnerError::is_retryable`),
  which is the classification an in-process consumer actually needs; no authorization-shaped
  error variant is added, because there is no authorization decision to report.

### Confirmation

* The posture is stated on the trait, which is what a consumer reads, rather than only in this
  file — the failure mode being corrected is that the contract was silent.
* Code review rule: a check against `ctx` appearing in this gear requires an ADR superseding this
  one, naming the policy source it consults. A check with no policy source is theatre.
* `#![forbid(unsafe_code)]` on both crates, unrelated to authorization but part of the same
  hardening pass, is asserted by compilation.

## Pros and Cons of the Options

### Trusted in-process primitive, non-enforcement documented

Chosen. Keep the parameter, state plainly that nothing enforces it.

* Good, because it matches what the gear actually is — no tenant-scoped resource, no policy
  source, no network surface
* Good, because it is consistent with every sibling gear in the same category
* Good, because it costs nothing to adopt and breaks no consumer
* Good, because it preserves trace propagation and leaves enforcement available without another
  contract version
* Neutral, because it relies on documentation to convey a property the type system cannot
* Bad, because a reader who skims signatures and not doc comments can still misread the
  parameter — mitigated by putting the note at the impl block too, where the `_ctx` binding is
* Bad, because "trusted" is a property of the deployment, not of the code, and a future
  deployment could violate the assumption without anything failing

### Enforcing gear

Authorize model access per call, enabling per-tenant model restrictions.

* Good, because it would make the parameter mean what it appears to mean
* Good, because per-tenant model restrictions are a plausible future requirement for a shared
  inference gear
* Bad, because there is no policy source to consult, so the first version would necessarily be a
  placeholder that always allows — which is exactly the misleading state being corrected
* Bad, because it is substantially more than the quarter-day budgeted; it needs a permission
  model, an administration path, and its own tests
* Bad, because it authorizes at the wrong layer: the tenant-facing operation is the consumer's,
  and re-checking here would either duplicate that decision or contradict it
* Bad, because it puts a policy lookup on the request path of a latency-sensitive compute
  primitive

### Remove the parameter

Delete `ctx` from all four methods.

* Good, because it is honest by construction rather than by documentation — nothing can be
  misread if nothing is there
* Good, because it makes the trait smaller
* Bad, because the trait is versioned by name: this is `OnnxRunnerClientV1V2`, a breaking change
  to a contract with a live consumer, bought for a clarity a doc comment also achieves
* Bad, because it forfeits trace propagation, the one thing the parameter currently earns
* Bad, because re-adding it later is another version, so it trades a cheap present option for a
  costly future one

### Leave it as it was

Undocumented `_ctx` bindings.

* Neutral, because it is what shipped, and nothing was broken by it
* Bad, because the posture is then inferred rather than known, and the inference people make is
  the unsafe one — that a successful call was an authorized call
* Bad, because it invites a well-meaning contributor to add enforcement with no policy behind it
* Bad, because it is the specific defect the release bar names: a gear whose authorization
  posture is unstated

## More Information

The general principle worth extracting: a parameter that is accepted and ignored is a claim the
code does not honour. There are only three defensible states — enforce it, remove it, or say
plainly that it is not enforced and why. Which of the three is right depends on whether the
component owns something worth guarding, and this one does not.

It is also worth being precise about what "trusted" means here, because the word does a lot of
work. It is not a claim that the gear's inputs are safe — they are not, which is why every call
is bounded in size, time and concurrency
(`cpt-cf-onnx-runner-principle-bounded-work`). It is a claim about *who can call it*: only code
in the same process, wired at build time. The bounds work exists precisely because a trusted
caller can still be a buggy or overloaded one.

## Traceability

- **DESIGN**: [DESIGN.md](../DESIGN.md) §3.3, §3.5, §5 item 12

This decision directly addresses the following requirements or design elements:

* `cpt-cf-onnx-runner-contract-client-v1` — the contract whose authorization posture this states,
  and whose versioning rule makes documenting cheaper than removing
* `cpt-cf-onnx-runner-constraint-no-caller-paths` — the constraint that removes the attack
  surface authorization would otherwise guard, and therefore the main reason this posture is
  defensible
* `cpt-cf-onnx-runner-constraint-no-http-surface` — no network boundary, so no untrusted caller
* `cpt-cf-onnx-runner-principle-bounded-work` — what protects the gear instead of authorization
* `cpt-cf-onnx-runner-principle-no-payload-in-telemetry` — bounds what may be taken from `ctx`
  into spans
* `cpt-cf-onnx-runner-feature-sdk-contract` — the feature this decision completes
