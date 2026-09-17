---
status: accepted
date: 2026-08-28
decision-makers: onnx-runner gear maintainers
---

# Commit a generated 285-byte ONNX graph, so a real session runs in CI

**ID**: `cpt-cf-onnx-runner-adr-test-fixture-strategy`

<!-- toc -->

- [Context and Problem Statement](#context-and-problem-statement)
- [Decision Drivers](#decision-drivers)
- [Considered Options](#considered-options)
- [Decision Outcome](#decision-outcome)
  - [Consequences](#consequences)
  - [Confirmation](#confirmation)
- [Pros and Cons of the Options](#pros-and-cons-of-the-options)
  - [A generated ONNX graph, committed](#a-generated-onnx-graph-committed)
  - [Build the graph at test time with ort's Model Editor](#build-the-graph-at-test-time-with-orts-model-editor)
  - [Feature-gate everything behind a fetched real model](#feature-gate-everything-behind-a-fetched-real-model)
  - [Generate the fixture with Python's onnx tooling](#generate-the-fixture-with-pythons-onnx-tooling)
- [What the offset test found](#what-the-offset-test-found)
- [What is deliberately not tested](#what-is-deliberately-not-tested)
- [Traceability](#traceability)

<!-- /toc -->

## Context and Problem Statement

The gear had 7 tests, all in `domain/model_registry.rs` — the one file with no dependencies
and no I/O. Session construction, checksum verification against a real file, graph-contract
checking against a real graph, tensor marshalling, dtype extraction and tokenization were
all untested. `cargo test -p cf-gears-onnx-runner` passing meant very little.

Stages 1 through 3 each added model-backed tests, and each put them behind a
`real-model-tests` feature, because the only available models are 528 MB, gitignored, and
fetched on demand. So the coverage existed but CI never ran it — which is the same place the
gear started, with more code.

The blocker was always the same: **there was no small ONNX graph to test against.**

## Decision Drivers

* CI must exercise a real session. Everything that fails in production starts from a file on
  disk being opened by ONNX Runtime.
* The default suite must not download anything. A test that reaches the network is a bug.
* No Python (`cpt-cf-onnx-runner-constraint-no-python`). ONNX's own authoring tooling is
  Python, so the obvious way to produce a fixture is closed.
* Whatever is committed must be regenerable by anyone. An opaque binary blob nobody can
  reproduce is a liability that outlives its author.
* The fixture must make contract verification and typed extraction *meaningful*, which means
  more than one tensor name and more than one dtype.
* Results must be bit-stable across the macOS dev machine and Linux CI, or the suite needs
  tolerances — and tolerances hide drift.

## Considered Options

* A generated ONNX graph, committed
* Build the graph at test time with `ort`'s Model Editor API
* Feature-gate everything behind a fetched real model (the status quo)
* Generate the fixture with Python's `onnx` tooling

## Decision Outcome

Chosen option: **a generated ONNX graph, committed**, at 285 bytes.

`examples/generate_test_fixture.rs` declares the graph in readable Rust and emits it with
`prost` — ONNX is protobuf, and the subset a graph this small needs is a few dozen message
fields. The generator *is* the provenance: the artifact is reproducible byte-for-byte, and
the file that produces it is reviewable in a way the artifact is not.

The graph takes `ids` (i64) and `weights` (f32) and echoes them to `ids_out` and `scores`
through `Identity` nodes, with symbolic batch and sequence dimensions. Two names in, two
names out, two dtypes — so a test can declare a name the graph lacks, or a dtype it does not
emit, and see the right failure. `Identity` makes the graph bit-stable everywhere, so tests
assert exact equality rather than choosing a tolerance.

**On committing a model artifact.** The constraint against this
(`cpt-cf-onnx-runner-constraint-model-artifacts-not-vcs`) is written about *size*: "nothing
that size enters version control", of artifacts totalling 528 MB. At 285 bytes this is
consistent with that reasoning rather than an exception to it, and the feature file's framing
of it as "an explicit exception" overstated the conflict.

### Consequences

* The default `cargo test -p cf-gears-onnx-runner` now loads and runs a real ONNX session:
  **42 tests, 0.05 s of execution**, no network, no files outside the repo.
* CI runs that suite on every PR touching this gear, and additionally compiles the
  `real-model-tests` suites with `--no-run` so they cannot bit-rot behind a flag nothing
  builds.
* The real-model suites stay feature-gated and stay out of CI. They are not redundant: they
  are the only tests that exercise a several-hundred-megabyte model, real tokenization, and
  the memory behaviour of a genuine session pool.
* **A new failure mode had to be closed.** Once the default suite loads a session, a
  developer with `ORT_DYLIB_PATH` unset gets `ort`'s hang rather than a failure. The fixture
  helpers therefore check for a runtime first and fail immediately with the command that
  fixes it. Without that, adding these tests would have made `cargo test` worse for anyone
  who had not read the README.
* `prost` becomes a dev-dependency of the gear, used only by the generator example.
* The fixture's SHA-256 is duplicated in `tests/common/mod.rs`, so regenerating the graph
  means updating it there. The generator prints the new digest, and a test verifies the
  committed file against it — so the duplication is self-correcting rather than a trap.

### Confirmation

* `the_fixture_stays_small` fails if committed fixtures exceed 100 KB.
* `loads_and_verifies_the_real_checksum` runs with `skip_checksum: false` against the real
  digest, so verification is exercised rather than bypassed.
* Every `Err` path in the load sequence has a test asserting on the **typed**
  `ModelFailureReason`, not on message substrings: checksum mismatch, unreadable file,
  contract mismatch on inputs and on outputs, a declared dtype the graph does not emit, and
  a declared output with no dtype at all.
* `round_trips_both_dtypes_exactly` asserts exact equality on both dtypes — available only
  because the graph is `Identity`.
* `every_replica_in_a_pool_serves` drives 9 calls through a 3-replica pool and asserts the
  pool is intact afterwards.

## Pros and Cons of the Options

### A generated ONNX graph, committed

Chosen. 285 bytes, produced by a Rust example using `prost`.

* Good, because the default suite exercises a real session with no download — the entire
  point
* Good, because the generator makes the artifact reproducible and reviewable; the graph is
  defined in prose-legible Rust rather than hidden in bytes
* Good, because `Identity` gives bit-stability, so assertions are exact
* Good, because it needs no Python and no new runtime dependency
* Neutral, because it commits a binary artifact, which the size-based constraint permits at
  this scale
* Bad, because the ONNX field numbers in the generator are hand-transcribed from the
  specification; a mistake there produces a file ONNX Runtime rejects — caught immediately,
  but only because a test loads it
* Bad, because the fixture's digest lives in two places

### Build the graph at test time with ort's Model Editor

`ort` 2.0.0-rc.12 exposes `Graph`, `Node`, `Model` and `Model::into_session`.

* Good, because nothing is committed at all
* Good, because the graph is defined in Rust, as with the chosen option
* Bad, and decisively: `into_session` hands back a `Session` directly. It cannot serialize.
  A session built that way **bypasses `LoadedModel::load` entirely** — the file read, the
  SHA-256 verification and the graph-contract check all begin with a path on disk, and those
  are precisely what needs testing
* Bad, because it is gated behind an `api-22` editor API whose availability varies with the
  runtime build, adding a second way for the suite to be unavailable

### Feature-gate everything behind a fetched real model

The status quo through Stages 1-3.

* Good, because it costs nothing to author
* Good, because the tests are real in the strongest sense
* Bad, because CI either downloads 528 MB or does not run them — and "does not run" is where
  the gear already was
* Bad, because feature-gated code that nothing builds rots silently
* Bad, because a 446 MB model takes ~14 s per load, so the suite is minutes rather than
  milliseconds, and loading several in one process turned out to trip a race in ONNX
  Runtime's own teardown

### Generate the fixture with Python's onnx tooling

The conventional approach.

* Good, because `onnx.helper` makes graph construction trivial and well-trodden
* Bad, because it contradicts `cpt-cf-onnx-runner-constraint-no-python`, a constraint this
  program paid to establish by removing a Python sidecar rather than wrapping it
* Bad, because it puts a Python toolchain between a developer and regenerating a test
  fixture, which is exactly the kind of friction that leads to nobody regenerating it

## What the offset test found

`infra/tokenizer.rs` carried a written warning that its offsets were unverified against
multi-byte input, and the feature file flagged the consequence: if they were character
offsets, every redaction span in the scanning pipeline would be silently wrong for non-ASCII
text, and the finding would need escalating across the program.

**They are byte offsets. The contract holds.** Measured against the fixture tokenizer:
`привет` is 6 characters and 12 bytes and returns `(0, 12)`; `日本` is 2 characters and 6
bytes and returns `(0, 6)`. Slicing the original text by the returned offsets reproduces each
token exactly, including across combining sequences.

The warning comment has been replaced by a reference to the tests. One of those tests
demonstrates that a character-offset implementation would produce a *shorter, still-valid*
slice — `при` rather than `привет` — which is why this was worth proving rather than
assuming: the wrong implementation does not crash, it quietly returns the wrong span.

## What is deliberately not tested

* **Model quality or detection accuracy.** Not this gear's concern, ever.
* **Throughput and concurrency scaling.** The correctness of concurrent execution is tested;
  its *performance* belongs to the benchmark in
  execution configuration.
* **Real tokenizer vocabularies.** The fixture tokenizer is a minimal WordLevel model. It
  proves the offset contract and the special-token distinction; it says nothing about how a
  real BPE vocabulary segments text, which is the model author's concern.
* **Cross-platform numerical agreement.** `Identity` sidesteps it. A fixture that computed
  something would need a tolerance, and the first thing a tolerance hides is a real
  platform difference.
* **The `ort` teardown race.** Loading and dropping several large sessions in one process
  occasionally aborts at exit, after assertions pass. It is upstream, it is not reproducible
  with the fixture, and a test that asserts on it would be asserting on a race.

## Traceability

- **DESIGN**: [DESIGN.md](../DESIGN.md) §1.2, §5 item 8

This decision directly addresses the following requirements or design elements:

* `cpt-cf-onnx-runner-phase-test-fixture` — the fixture and its generator
* `cpt-cf-onnx-runner-phase-test-infra` — `infra/` coverage, which the fixture unblocks
* `cpt-cf-onnx-runner-phase-test-offsets` — the offset contract, now proven
* `cpt-cf-onnx-runner-phase-test-ci` — the default suite running on every relevant PR
* `cpt-cf-onnx-runner-constraint-no-python` — upheld; it is why the generator exists
* `cpt-cf-onnx-runner-constraint-model-artifacts-not-vcs` — consistent with, not an
  exception to: the constraint is about size, and this is 285 bytes
