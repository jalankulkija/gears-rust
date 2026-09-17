Created:  2026-08-26 by Constructor Tech
Updated:  2026-08-28 by Constructor Tech

# Feature: Test Suite

<!-- toc -->

- [1. Feature Context](#1-feature-context)
- [2. Current State](#2-current-state)
- [3. Scope](#3-scope)
- [4. Phases](#4-phases)
- [5. Feature Acceptance Criteria](#5-feature-acceptance-criteria)
- [6. Non-Functional Considerations](#6-non-functional-considerations)
- [7. Outcome](#7-outcome)

<!-- /toc -->

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-featstatus-test-suite`

## 1. Feature Context

- [x] `p1` - `cpt-cf-onnx-runner-feature-test-suite`

### Overview

Cover the two-thirds of the gear that is currently untested, prove the tokenizer's offset
contract, and make all of it run in CI without downloading half a gigabyte.

**Closes**: [DESIGN.md](../../DESIGN.md) §5 item 8.

### Purpose

The gear has 7 tests, all in `domain/model_registry.rs` — the one file with no dependencies
and no I/O. Everything that can actually fail in production is untested: session
construction, checksum verification against a real file, graph-contract checking against a
real graph, tensor marshalling, dtype extraction, and tokenization.

The comparison worth internalizing: `file-parser`, the closest analogue in this repo, has
2,339 lines of integration tests across eight files, including a dedicated timeout test and
a path-traversal test. That is the standard this gear is being measured against.

One documented risk makes tokenizer testing urgent rather than merely overdue. From
`infra/tokenizer.rs`:

> **Verify before trusting in production**: [...] do not assume the two behave the same
> without an actual round-trip test against a real `tokenizer.json` and known
> multi-byte-character input once one is available.

Every consumer that maps a model-space span back to source text depends on those offsets
being byte offsets. If they are character offsets, every redaction span in the scanning
pipeline is silently wrong for any non-ASCII input. That test does not exist.

### References

- **Gear design**: [DESIGN.md](../../DESIGN.md) §1.2, §5
- **In-repo reference**: `gears/file-parser/file-parser/tests/` — eight suites, and
  `magika_timeout_tests.rs` for testing an ONNX session's failure modes
- **Repo standards**: `docs/toolkit_unified_system/12_unit_testing.md`,
  `docs/toolkit_unified_system/13_e2e_testing.md`
- **CI**: `.github/workflows/ci.yml` already provisions onnxruntime and sets `ORT_DYLIB_PATH`

## 2. Current State

**Implemented.** The table below is the state before this feature landed; see §7 for what it
is now.

| Area | Tests |
|---|---|
| `domain/model_registry.rs` | 7 |
| `infra/ort_session.rs` | 0 |
| `infra/tokenizer.rs` | 0 |
| `gear.rs` (init, client dispatch) | 0 |
| `onnx-runner-sdk` | 0 |
| Integration (`tests/`) | 0 — the directory does not exist |

There is no test fixture model. The two models on disk total 528 MB, are gitignored, and are
fetched by `make onnx-models-fetch`. Nothing in CI runs this gear's code against a real
session, so `cargo test -p cf-gears-onnx-runner` passing means very little.

## 3. Scope

**In scope:**

- A tiny committed ONNX fixture model, and the decision of how to produce it.
- Unit coverage for `infra/` behavior that does not need a real graph.
- Integration coverage for load, verification, inference, and tokenization against the
  fixture.
- Tokenizer offset correctness against multi-byte input.
- CI wiring that needs no large download.

**Out of scope:**

- Concurrency and throughput benchmarks — [multi-model-execution](./multi-model-execution.md)
  Phase 4 owns the benchmark; this feature owns the *correctness* tests that run beside it.
- Model detection quality. Not this gear's concern, ever.
- The program-level E2E harness, which exercises the whole scanning pipeline
  (`docs/genai-security/features/e2e-test-harness.md`).

## 4. Phases

### Phase 1: Fixture Model

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-phase-test-fixture`

**Decision to make and record.** Three options for a testable graph:

| Option | Cost | Note |
|---|---|---|
| Commit a hand-built minimal ONNX graph (e.g. identity/add over declared inputs), a few KB | Small, needs a generator script + provenance note | Preferred — deterministic, no download, tests exact tensor semantics |
| Generate the graph at test time in Rust | No artifact to commit; more code, and `ort` is an inference runtime, not a graph builder | Viable only if a small builder crate is acceptable |
| Depend on a fetched real model, gate tests behind a feature flag | Zero authoring cost | Rejected as the primary path: CI either downloads 528 MB or the tests do not run, and "does not run" is where we already are |

**Requirements**

1. A fixture model of a few kilobytes, committed (an explicit exception to
   `cpt-cf-onnx-runner-constraint-model-artifacts-not-vcs`, justified by size and recorded),
   with a matching minimal `tokenizer.json`.
2. A documented, re-runnable way to regenerate it, plus its sha256 in the test config.
3. Its graph exercises more than one input and output name, and at least two dtypes, so
   contract verification and typed extraction are both meaningful.
4. A test helper that builds a `ModelConfig` pointing at the fixture, usable from every
   suite.

**Acceptance criteria**

- [x] `cargo test -p cf-gears-onnx-runner` loads a real session with no network access and no
      files outside the repo.
- [x] The fixture's provenance and regeneration procedure are documented next to it —
      `tests/fixtures/README.md`, plus the generator itself.
- [x] Total committed fixture bytes are under 100 KB — 285 bytes of ONNX plus ~1 KB of
      tokenizer, asserted by `the_fixture_stays_small`.

### Phase 2: `infra/` Coverage

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-phase-test-infra`

**Requirements**

1. Load path: success; checksum mismatch; `skip_checksum` honored; unreadable file; missing
   tokenizer; contract mismatch on inputs and on outputs.
2. Inference path: round-trip for each `TensorData` variant; multi-input and multi-output;
   output name missing from session results; declared/actual dtype mismatch.
3. Replica behavior: a model with `replicas: 3` distributes calls across replicas
   (observable through the selector), and concurrent calls succeed.
4. Tokenizer: `encode` returns consistent ids/mask/offsets/special-token-mask lengths;
   `encode_fragment` adds no special tokens; empty input; input at the size limit.
5. Every error variant the gear can return is produced by at least one test.

**Acceptance criteria**

- [x] Every `Err` path in the load sequence is reached by a test: checksum mismatch,
      unreadable file, contract mismatch on inputs and outputs, a declared dtype the graph
      does not emit, a declared output with no dtype, and a missing tokenizer.
- [x] No test in the default suite depends on a fetched model.
- [x] Tests assert on typed variants (`ModelFailureReason`, `OnnxRunnerError`), not on
      message substrings — message content is asserted only where the message *is* the
      contract, e.g. that an error names the offending tensor.

### Phase 3: Tokenizer Offset Correctness

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-phase-test-offsets`

**Requirements**

1. A round-trip test over text with multi-byte characters — Cyrillic, CJK, an emoji with a
   combining sequence — asserting that slicing the *original* text by each returned offset
   pair yields the token's own text.
2. Explicit assertions that offsets are **byte** offsets, per `TokenEncoding`'s documented
   contract.
3. A test for text where byte length and character length differ substantially, so a
   character-offset implementation cannot pass by coincidence.
4. If the offsets turn out to be character offsets: stop, do not "fix" the test. Record the
   finding, fix the contract or the conversion, and notify `onnx-scanner` — every redaction
   span in the pipeline depends on it.

**Acceptance criteria**

- [x] Slicing by returned offsets reproduces token text exactly for all fixture inputs,
      across Cyrillic, CJK, combining sequences and a long mixed string.
- [x] The warning in `infra/tokenizer.rs` is replaced by a reference to the tests, and by
      the measured result.
- [x] The teeth are proven by an executable test rather than a manual experiment:
      `a_character_offset_implementation_would_fail_these_tests` shows that char offsets
      applied to the same text yield `при` instead of `привет` — a shorter, still-valid
      slice. Written as a test so it stays proven, instead of being done by hand and
      reverted.

### Phase 4: CI Integration

- [x] `p1` - **ID**: `cpt-cf-onnx-runner-phase-test-ci`

**Requirements**

1. The suite runs in the standard CI job for this gear, with no model download.
2. `ORT_DYLIB_PATH` / onnxruntime provisioning already present in `ci.yml` is confirmed
   sufficient for this gear's tests; if not, extended in the same change.
3. Optional feature-gated suites for the real models (`--features real-models`) run against a
   fetched model set, for local and nightly use, and are skipped cleanly when the files are
   absent — never silently passing.
4. Test execution time for the default suite stays within the gear's CI budget; recorded.

**Acceptance criteria**

- [x] A PR touching only this gear runs its full default suite in CI — the change filter
      covers `gears/onnx-runner/**`, and the job runs `cargo test -p cf-gears-onnx-runner`.
- [x] The real-model suites are demonstrated green locally against the fetched set, and CI
      now compiles them with `--no-run` so they cannot rot behind their flag.
      *Partial on skipping:* they are gated at compile time by a cargo feature, so with the
      feature off they do not appear in the run at all. Enabling the feature without the
      models fails loudly. Making the absence itself loud would need runtime gating rather
      than a feature — noted, not done.
- [x] Default suite wall-clock is recorded in the ADR and in §7.

## 5. Feature Acceptance Criteria

- [x] `infra/` is no longer a blind spot: session load, verification, inference, dtype
      extraction and tokenization are all covered.
- [x] A real ONNX session is loaded and executed in CI on every relevant PR.
- [x] The tokenizer offset contract is proven, not asserted in a comment. **They are byte
      offsets** — see §7.
- [x] Every error variant reachable from the load and validation paths is covered.
- [ ] Concurrency, panic-recovery and cancel-safety tests from
      [execution-safety](./execution-safety.md) run in the **same** suite. They run, and now
      compile in CI, but still need the real model and so stay feature-gated. Porting them
      to the fixture is the obvious follow-up and was not done here.
- [x] ADR written: fixture strategy and what is deliberately not tested —
      [ADR 0007](../../ADR/0007-cpt-cf-onnx-runner-adr-test-fixture-strategy.md).

## 6. Non-Functional Considerations

**Determinism.** Inference over a fixed fixture graph with fixed inputs must be bit-stable
across platforms, or assertions use tolerances chosen deliberately. Note which, and why.

**CI cost.** The default suite must not download models; that is the whole point of the
fixture. Guard against regression: a test that reaches the network is a bug.

**Cross-platform.** onnxruntime provisioning differs between the macOS dev machine and Linux
CI. The suite must pass on both, and the fixture must not encode a platform-specific
optimization level.

**Honesty.** A skipped feature-gated suite must be loudly skipped. Silent skipping produces
green builds that prove nothing, which is worse than a red one.

## 7. Outcome

**The fixture is generated, not authored.** `examples/generate_test_fixture.rs` declares a
two-input, two-output graph in readable Rust and emits it with `prost` — ONNX is protobuf,
and no Python is involved. The result is 285 bytes. `ort`'s Model Editor API was rejected
because it produces a `Session` and cannot serialize: a session built that way bypasses
`LoadedModel::load`, and the file read, checksum and contract check are exactly what needed
testing. Full reasoning in [ADR 0007](../../ADR/0007-cpt-cf-onnx-runner-adr-test-fixture-strategy.md).

**Coverage now:**

| Area | Before | After |
|---|---|---|
| `domain/model_registry.rs` | 7 | 18 |
| `infra/` (load, inference, dtype, tokenizer) | 0 | 24 in the default suite |
| Integration (`tests/`) | 0 — no directory | 5 suites |
| Runs in CI | nothing model-backed | the whole default suite |

**42 tests, 0.05 s of execution, ~25 s wall clock** including compilation — against 59 s for
a single real-model suite, which is the entire argument for the fixture.

**The offset question is answered: they are byte offsets.** `привет` is 6 characters and 12
bytes and returns `(0, 12)`; `日本` is 2 characters and 6 bytes and returns `(0, 6)`. Slicing
by the returned offsets reproduces every token exactly. No cross-program escalation is
needed, and the warning comment in `infra/tokenizer.rs` is now a pointer to the tests.

Worth recording *why* this needed proving: a character-offset implementation would not have
crashed. It would have returned `при` where `привет` was meant — a shorter, still-valid
slice. Every redaction span in the pipeline would have been quietly wrong for non-ASCII
input.

**One new failure mode had to be closed.** A default suite that loads a session means a
developer with `ORT_DYLIB_PATH` unset meets `ort`'s hang rather than a failure. The fixture
helpers check for a runtime first and fail immediately with the command that fixes it —
without that, adding these tests would have made `cargo test` worse for anyone who had not
read the README.

**Left for later:** the concurrency, panic-recovery and cancel-safety suites still need the
real model. Porting them to the fixture would bring them into CI too, and is the natural
next step for this feature.
