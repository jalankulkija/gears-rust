---
status: accepted
date: 2026-08-26
decision-makers: onnx-runner gear maintainers
---

# Keep `onnx-runner` a generic tensor-and-tokenizer layer with no knowledge of model families

**ID**: `cpt-cf-onnx-runner-adr-generic-tensor-layer`

<!-- toc -->

- [Context and Problem Statement](#context-and-problem-statement)
- [Decision Drivers](#decision-drivers)
- [Considered Options](#considered-options)
- [Decision Outcome](#decision-outcome)
  - [Consequences](#consequences)
  - [Confirmation](#confirmation)
- [Pros and Cons of the Options](#pros-and-cons-of-the-options)
  - [A — Thin generic layer](#a--thin-generic-layer)
  - [B — Model-family-aware layer](#b--model-family-aware-layer)
  - [C — One gear per model family](#c--one-gear-per-model-family)
  - [D — No gear; each consumer embeds `ort` itself](#d--no-gear-each-consumer-embeds-ort-itself)
- [More Information](#more-information)
- [Traceability](#traceability)

<!-- /toc -->

## Context and Problem Statement

`prompt-scanner` needs local ML inference for PII detection. The candidate models come from
two structurally different families: GLiNER-style zero-shot span classifiers, which require a
schema/label prompt to be encoded into the input and produce span probabilities plus a count
gate, and plain BIO token classifiers, which have a fixed label set baked into the model and
produce per-token tag logits. Each family needs its own input construction and its own output
decoding.

Where should that family-specific knowledge live — inside the gear that owns the ONNX session,
or in the consumer that asked for the inference? This is the gear's defining boundary
question, and answering it wrong makes every future model a change to shared infrastructure.

## Decision Drivers

* A second model family was already known to be coming (prompt-injection detection), and its
  decode path is not the PII model's decode path. Whatever boundary we choose is immediately
  tested by it.
* Model families are a detection concern. `prompt-scanner` owns detection; a runtime gear that
  knows what `pii.email` means has acquired an opinion that belongs to its consumer.
* The gear should be reusable outside this program — embeddings, classification, ranking are
  all "run tensors through a graph", and none of them are DLP.
* `ort` 2.x churns across release candidates. Confining it to one file is only possible if that
  file has no reason to know about model semantics.
* A separate PII-detection benchmarking project reached the same boundary independently: it
  confines its `ort` integration to a single module and puts each model family in its own, for
  the same stated reason — an upgrade or a new family touches one file.
* Model *selection* is unsettled — the current PII models are declared placeholders. A boundary
  that makes swapping a model cheap is worth more than one that makes calling it convenient.

## Considered Options

* A — Thin generic layer: `run(model_id, tensors)` / `encode(model_id, text)`, no family concepts
* B — Model-family-aware layer: configurable "model kinds" (`gliner`, `bio`, …) inside the gear
* C — One gear per model family
* D — No gear at all; each consumer embeds `ort` itself

## Decision Outcome

Chosen option: **A — Thin generic layer**, because it is the only option under which adding a
model family is a change to one consumer rather than a change to shared infrastructure, and
the only one that leaves the gear usable for non-detection workloads. The gear owns model
lifecycle, session hosting, integrity verification, and tokenization; prompt construction,
output decoding, label sets, and thresholds are the consumer's.

### Consequences

* The public contract is deliberately narrow: `run`, `encode`, `encode_fragment`, `list_models`
  (`onnx-runner-sdk/src/client.rs`). Every one of them is expressible without naming a model
  family, and that is the acceptance test for any future addition to it.
* Family logic lives in the consumer. `onnx-scanner` carries `domain/gliner2/`
  (schema-prompt encoding, span decoding) and `domain/bio_token_classifier/` (BIO tag merging),
  and selects between them by its own config — the runner is not aware that either exists.
* `encode_fragment` — "tokenize without special tokens" — stays in the contract even though the
  need arose from GLiNER-family schema prefixes, because it is expressible as a generic
  tokenizer primitive. This is the boundary line in practice: the *capability* may be motivated
  by a family, but the *vocabulary* may not name one.
* The gear must not gain label sets, category taxonomies, confidence thresholds, or
  post-processing. A change request that needs any of them is a design failure to escalate, not
  to accommodate (`cpt-cf-onnx-runner-principle-generic-execution`).
* Consumers pay for the generality: `onnx-scanner` must build tensors and interpret outputs
  itself, which is more code there than option B would need. That cost is accepted and is the
  point of the trade.
* Tensor dtypes must be declared per model in configuration (`output_dtypes`), because a
  generic layer cannot infer meaning from a graph it refuses to understand. This is what makes
  the current silent F32 default a correctness bug rather than a convenience — see
  input validation and resource bounds Phase 3.
* The gear's real acceptance test is external and deferred to
  execution configuration: adding the prompt-injection
  model must require configuration changes only, with no `onnx-runner` source change. If that
  fails, this ADR's boundary was drawn in the wrong place and should be revisited rather than
  worked around.

### Confirmation

* No model-family identifier appears anywhere in the gear. Verified: `gliner`, `bio`, `pii`,
  and label-set strings occur in `gears/prompt-scanner/plugins/onnx-scanner/` only, and in no
  file under `gears/onnx-runner/`.
* `ort` is imported by exactly one file (`infra/ort_session.rs`) and `tokenizers` by exactly
  one (`infra/tokenizer.rs`) — the single-importer rule this boundary makes possible
  (`cpt-cf-onnx-runner-constraint-single-importer`).
* The config-only test above, when the second model family lands.

## Pros and Cons of the Options

### A — Thin generic layer

`run this named model with these named tensors, get these named tensors back`, plus a
HuggingFace tokenizer wrapper. Chosen.

* Good, because a new model family touches one consumer module and zero shared code
* Good, because the gear is reusable for embeddings, classification, and ranking — use cases
  that have nothing to do with DLP
* Good, because it keeps `ort` and `tokenizers` each behind a single importer, so a
  release-candidate upgrade is one file's problem
* Good, because it matches an independently-reached precedent in a comparable project
* Neutral, because the consumer/runner split means two crates must be understood to follow one
  inference end to end
* Bad, because every consumer re-implements tensor assembly and output decoding; two consumers
  of the same family would duplicate that work until someone factors out a shared helper
* Bad, because the gear cannot validate that a caller's tensors are *semantically* right — only
  that they match the declared graph contract

### B — Model-family-aware layer

Configurable `kind: gliner | bio | …` per model; the gear builds prompts and decodes outputs.

* Good, because consumers become trivial: pass text, receive spans or labels
* Good, because decode logic is written once for all consumers of a family
* Bad, because the next family requires changing the gear, which is shared infrastructure — the
  exact coupling this decision exists to prevent, and the reason a "generic ONNX runner" would
  stop being generic by its second model
* Bad, because the gear inherits detection semantics (labels, thresholds, span merging) that
  belong to `prompt-scanner`, blurring which component owns detection quality
* Bad, because `ort` and `tokenizers` would stop being confined to one file each, since decode
  paths need tensor introspection
* Bad, because model selection is unsettled: baking family support into the gear makes a model
  swap a shared-infrastructure change

### C — One gear per model family

`gliner-runner`, `token-classifier-runner`, etc.

* Good, because each gear stays small and independently testable
* Bad, because session hosting, replica pooling, checksum and graph verification, and
  tokenization get duplicated per family — the genuinely generic parts are exactly the parts
  that would be copied
* Bad, because N gears means N `ort` importers and N dependency upgrades
* Bad, because it multiplies operational surface (config, health, metrics) for no gain

### D — No gear; each consumer embeds `ort` itself

Every plugin loads its own sessions.

* Good, because it is the least indirection for a single consumer
* Bad, because model lifecycle, integrity verification, and thread/memory budgeting get
  reinvented per consumer, each with its own bugs
* Bad, because two consumers loading the same model would load it twice
* Bad, because it puts native-library and thread-pool management inside plugin gears, which is
  where it is hardest to reason about

## More Information

This decision was argued in the gear's original design sketch (option A vs option B) and
implemented in Milestone 1, but was never recorded — one of the reasons the gear's docs drifted
into claiming capabilities it did not have. It is written now, unchanged, as the first of the
gear's ADRs, because it is the constraint every other decision here is measured against.

The boundary is deliberately stricter than "no DLP knowledge": it is "no *model-family*
knowledge". A future request to add, say, automatic softmax over a named output should be
refused on the same grounds, even though softmax is not a DLP concept.

## Traceability

- **DESIGN**: [DESIGN.md](../DESIGN.md) §1.1, §3.1, §4 (decision 1)

This decision directly addresses the following requirements or design elements:

* `cpt-cf-onnx-runner-principle-generic-execution` — this ADR *is* that principle's rationale
* `cpt-cf-onnx-runner-contract-client-v1` — constrains what may be added to the contract: any
  new method must be expressible without naming a model family
* `cpt-cf-onnx-runner-constraint-single-importer` — only achievable because the gear has no
  reason to introspect tensors semantically
* `cpt-cf-genai-security-principle-generic-execution-layer` — the program-level statement of the
  same boundary
* `cpt-cf-onnx-runner-feature-multi-model` — carries this ADR's external acceptance test:
  a second model family added by configuration alone
