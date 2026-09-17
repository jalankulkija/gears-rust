---
status: accepted
date: 2026-08-26
decision-makers: onnx-runner gear maintainers
---

# Provision model artifacts out of band; the gear only loads and verifies files already on disk

**ID**: `cpt-cf-onnx-runner-adr-out-of-band-model-provisioning`

<!-- toc -->

- [Context and Problem Statement](#context-and-problem-statement)
- [Decision Drivers](#decision-drivers)
- [Considered Options](#considered-options)
- [Decision Outcome](#decision-outcome)
  - [Consequences](#consequences)
  - [Confirmation](#confirmation)
- [Pros and Cons of the Options](#pros-and-cons-of-the-options)
  - [Out-of-band provisioning with a dev/CI fetch script](#out-of-band-provisioning-with-a-devci-fetch-script)
  - [Gear fetches models at startup](#gear-fetches-models-at-startup)
  - [Commit model artifacts to version control](#commit-model-artifacts-to-version-control)
  - [Embed the model in the binary](#embed-the-model-in-the-binary)
- [More Information](#more-information)
- [Traceability](#traceability)

<!-- /toc -->

## Context and Problem Statement

The gear needs ONNX model files and their tokenizers on disk before it can serve anything. The
two current artifacts are large — `gliner2-pii` is 441 MB and `minilm-l6-pii` 87 MB, 528 MB
together — and they come from third-party HuggingFace repositories whose contents can change
under a mutable ref. How do those files get onto a machine, and what is the gear's own
responsibility in that process?

The question matters beyond convenience: a model file is executable-adjacent input. A swapped
or corrupted artifact silently changes what the detector detects, and in this program the
detector's output gates whether a user's prompt is blocked.

## Decision Drivers

* Model provenance is a supply-chain concern. A floating `main` ref means the bytes we tested
  are not necessarily the bytes we run.
* A corrupted or substituted artifact must fail loudly, not degrade detection quietly.
* The request path must not depend on network I/O, and neither should startup — a scanning
  backend that cannot start because HuggingFace is unreachable is worse than one that will not
  start because an operator has not staged a file.
* Deployments may be network-restricted. Requiring egress to a third-party model host at
  runtime constrains where the gear can run, for no operational benefit.
* Local development, CI, and testing still need a low-friction way to populate a model
  directory without a human clicking through a website.
* 528 MB of binary artifacts must not enter version control, and CI must not be made to
  download them on every run.
* The gear already refuses caller-supplied model paths
  (`cpt-cf-onnx-runner-constraint-no-caller-paths`); model *acquisition* should not reintroduce
  the arbitrary-source problem that constraint exists to remove.

## Considered Options

* Out-of-band provisioning, with a pinned-revision fetch script for dev/CI only
* The gear fetches missing models itself at startup
* Commit model artifacts to version control
* Embed the model in the binary with `include_bytes!`

## Decision Outcome

Chosen option: **out-of-band provisioning with a dev/CI fetch script**, because it is the only
option that keeps the gear's runtime free of network dependencies and third-party trust while
still giving developers and CI a reproducible one-command path to a working model directory.

In production, operators place verified files at a configured path. The gear loads them,
verifies SHA-256 against configured expectations, verifies the graph contract, and fails
startup on mismatch. It never fetches anything, at any time.

### Consequences

* `scripts/fetch-models.sh` exists purely for local dev, CI, and testing, exposed as
  `make onnx-models-fetch`. It is never invoked from gear startup or from the request path
  (`cpt-cf-onnx-runner-constraint-no-fetch-in-prod`).
* The script's manifest pins each file to an **immutable commit SHA** with a per-file SHA-256
  and byte size. Mutable refs are rejected outright — `main`, `master`, and `HEAD` cause the
  script to exit rather than resolve. There is no "latest".
* `ModelConfig.sha256` is a required field, verified at load. `skip_checksum` exists for local
  iteration only; enabling it in a deployment is a misconfiguration, and the field name is
  deliberately unattractive.
* `gears/onnx-runner/models/` is gitignored. Nothing derived from a model artifact may be
  committed either, with one narrow exception granted in
  the test suite Phase 1: a purpose-built fixture graph of a few
  kilobytes, whose provenance and regeneration procedure are documented next to it.
* CI therefore cannot exercise the real models by default — the reason the test strategy is
  built on a small committed fixture rather than on the shipped artifacts, with real-model
  suites feature-gated and skipped loudly when files are absent.
* Deployment tooling inherits an obligation: the Helm chart and Ansible role owed in Milestone 4
  must stage model files (volume, init container, or image layer) and document how an operator
  supplies and updates them. That is deployment work this decision creates, not avoids.
* Rotating a model is a two-step, reviewable change: update the manifest entry (new revision,
  new checksum) and the corresponding `sha256` in configuration. A mismatch between them fails
  startup, which is the intended tripwire.
* The gear takes no position on model licensing or provenance beyond checksums. That remains an
  open question upstream — `gliner2-pii` is an unaudited community export.

### Confirmation

* The gear has no HTTP client dependency: `onnx-runner/Cargo.toml` and
  `onnx-runner-sdk/Cargo.toml` contain no `reqwest`, `ureq`, `hyper`, or `curl`. Verified. This
  is the mechanical guarantee that the gear cannot fetch anything, and a review rule for any
  future dependency addition.
* `fetch-models.sh` refuses unpinned revisions — an explicit `main|master|HEAD` rejection with a
  non-zero exit.
* A checksum mismatch fails startup rather than warning; covered by
  the test suite Phase 2.
* Code review: no invocation of the script from Rust, and no `std::process::Command` in the
  gear.

One honest caveat, recorded because it is adjacent and easy to mistake for a violation: the
`ort` dependency does use `download-binaries`, which fetches the **ONNX Runtime library** at
*build* time. That is a build-time dependency on a Microsoft release, not a runtime model
fetch, and it is a separate decision with its own consequences, tracked separately.

## Pros and Cons of the Options

### Out-of-band provisioning with a dev/CI fetch script

Operators stage files; the gear loads and verifies. A pinned-revision script serves dev and CI.
Chosen.

* Good, because startup and the request path require no network access, so the gear runs in
  restricted environments unchanged
* Good, because pinned revisions plus checksums make the running bytes provably the tested bytes
* Good, because a substituted artifact fails startup loudly instead of altering detection
  silently
* Good, because developers get one command and no manual downloads
* Neutral, because it moves work to deployment tooling, which has to stage large files
* Bad, because first-time setup is not zero-touch: a fresh environment with an empty models
  directory fails to start until someone provisions it
* Bad, because model rotation requires coordinated edits in two places (manifest and config)

### Gear fetches models at startup

Download missing models on boot, verifying checksums.

* Good, because zero-touch setup everywhere, including new environments
* Good, because it could still verify checksums after download
* Bad, because it makes startup depend on third-party host availability — an outage at
  HuggingFace becomes an outage of the scanning backend
* Bad, because it requires network egress from the process that handles sensitive content,
  widening the trust boundary for no operational gain
* Bad, because it needs an HTTP client, retry policy, partial-download handling, disk-space
  management, and concurrency control across replicas — a meaningful subsystem inside a gear
  whose job is inference
* Bad, because in a restricted or air-gapped environment it fails in a way that looks like a
  bug rather than a configuration gap

### Commit model artifacts to version control

Track the 528 MB in git (or Git LFS).

* Good, because provisioning becomes automatic and reproducible with no extra mechanism
* Good, because the exact bytes are pinned by the commit
* Bad, because 528 MB in git history is permanent and penalizes every clone forever
* Bad, because Git LFS adds infrastructure and credentials to every consumer of the repository
* Bad, because model rotation rewrites large binaries repeatedly
* Bad, because it embeds third-party artifacts of unclear licensing into our repository, which
  is exactly the question OQ-1 has not answered yet

### Embed the model in the binary

`include_bytes!` the model, as `file-parser`'s Magika dependency does.

* Good, because there is nothing to provision and nothing to verify at runtime — the artifact
  cannot drift from the binary
* Good, because it is a proven pattern already present in this workspace
* Neutral, because it works well at Magika's scale (~2.8 MB)
* Bad, because it does not work at ours: a 441 MB model produces a build artifact nobody wants
  to compile, ship, or cache
* Bad, because it makes models a compile-time choice, so swapping a model or hosting a second
  one requires a rebuild — incompatible with operator-configured models
  (`cpt-cf-onnx-runner-contract-model-config`)

## More Information

The pinned-revision-plus-checksum pattern in `fetch-models.sh` follows a comparable
PII-detection benchmarking project's own script, which arrived at the same posture for the
same reason.

This decision was made during Milestone 1 and implemented, but never recorded. It is written
now unchanged. The part that remains genuinely open is not *how* artifacts arrive but *whether
we are allowed to ship these particular ones* — model licensing and provenance, OQ-1.

## Traceability

- **DESIGN**: [DESIGN.md](../DESIGN.md) §3.5, §4 (decision 4)

This decision directly addresses the following requirements or design elements:

* `cpt-cf-onnx-runner-constraint-no-fetch-in-prod` — this ADR is that constraint's rationale
* `cpt-cf-onnx-runner-constraint-no-caller-paths` — acquisition is operator-side, so no
  arbitrary source is reachable from a caller or from the gear itself
* `cpt-cf-onnx-runner-constraint-model-artifacts-not-vcs` — the size and licensing argument for
  keeping `models/` gitignored
* `cpt-cf-onnx-runner-principle-fail-at-load` — checksum verification at load is the mechanism
  that makes out-of-band provisioning safe rather than merely convenient
* `cpt-cf-onnx-runner-seq-load` — the load sequence this decision defines the preconditions for
* `cpt-cf-genai-security-constraint-no-model-fetch-in-prod` — the program-level statement of the
  same constraint
* `cpt-cf-onnx-runner-feature-test-suite` — inherits the consequence that CI cannot depend on
  the real artifacts, hence the committed fixture model
