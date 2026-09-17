Created:  2026-08-31 by Constructor Tech
Updated:  2026-09-02 by Constructor Tech

# Detection-quality evaluation — design

A companion to [ADR 0009](../ADR/0009-cpt-cf-onnx-runner-adr-replica-pool-sizing.md), which measures how fast each candidate
model runs and says nothing about whether it finds anything. This document is the plan for the other half.

This is the design and the survey. **Measured results are in
[QUALITY-RESULTS.md](./QUALITY-RESULTS.md)** — four of the seven models are
scored against REDACT as of 2026-09-02.

**Why it matters:** the performance work found a 12x throughput spread across
seven PII models. That spread is only actionable if the fast models detect
comparably well, and a model that is 12x faster and misses PII is worse than
useless. No detection-quality comparison exists for any of them.

<!-- toc -->

- [1. Two questions, two kinds of data](#1-two-questions-two-kinds-of-data)
- [2. Existing benchmarks](#2-existing-benchmarks)
- [3. Data we should not score on](#3-data-we-should-not-score-on)
- [4. The taxonomy problem](#4-the-taxonomy-problem)
- [5. Negative controls](#5-negative-controls)
- [6. Injecting PII into public-domain text](#6-injecting-pii-into-public-domain-text)
- [7. Metrics](#7-metrics)
- [7a. What to measure through](#7a-what-to-measure-through)
- [8. An anecdote, recorded as such](#8-an-anecdote-recorded-as-such)
- [9. References](#9-references)

<!-- /toc -->

## 1. Two questions, two kinds of data

They are usually conflated, and they should not be: one is cheap and one is not.

| | Does real text change the *timing*? | How well does each model *detect*? |
|---|---|---|
| Needs gold labels | no | yes |
| Needs a shared taxonomy | no | yes |
| Needs multiple languages | for tokenizer behaviour | yes |
| Effort | days | weeks, mostly mapping |

**The timing question needs no labels at all.** It needs realistic token-length
*distributions* and realistic token *values*, both obtainable from any
multilingual corpus: tokenize with each model's own `tokenizer.json`, record the
length histogram, and replay real id sequences.

Two reasons this is not merely tidying up:

- The harness currently synthesizes `input_ids` as a constant (`1` at every
  position) at a single fixed length. Real traffic is a distribution, and the
  capacity table reports one point on it.
- Three of the seven models contain **data-dependent operators**, so for them the
  standing assumption that timing follows shapes rather than values is
  unverified:

  | Model | Operator |
  |---|---|
  | `gliner-pii-edge` | `NonZero` x3 |
  | `gliner-pii-base` | `NonZero` x3 |
  | `multilang-pii-ner` | `Loop` x1 |

  `NonZero` produces an output whose *size* depends on the values fed in, and
  everything downstream inherits that. The GLiNER cases are most likely consuming
  `words_mask`, which the harness sets to its maximum — but that is reasoning from
  intent, not from tracing the graph.

## 2. Existing benchmarks

Surveyed 2026-08-31. **Licence terms below are from a first reading and must be
confirmed against the current upstream text before anything is committed to.**

| Suite | Size | Languages | Types | Construction | Eval code |
|---|---|---|---|---|---|
| [REDACT](https://github.com/guneeshvats/REDACT-PII-Benchmark) | 13,427 records / 324,078 annotations | 25, 9 scripts | 51 canonical | controlled synthesis | yes |
| [RedactionBench](https://arxiv.org/abs/2606.18782) | 200 documents | multilingual | contextual | human-annotated, real-seeded | yes |
| [presidio-research](https://github.com/microsoft/presidio-research) | generator, not a corpus | template-driven | configurable | templates + fake providers | yes |

### REDACT — the primary candidate

Records are synthesized with controlled variation across nine axes — domain,
format, difficulty, length, density, code-switching, language, adjacency and
co-occurrence pattern — under a set of stated hard rules, with 453-621 records per
language. It ships scoring code supporting exact, partial and fuzzy span matching,
reporting micro-F1, macro-F1, per-type breakdowns and recall stratified by
disclosure form and sensitivity tier. There is a locked 1000-record evaluation
split, which is what a gate should be run against.

The controlled axes are the part that cannot be cheaply reproduced. They are what
turns "this model scores 0.81" into "this model fails on code-switched text and on
high-density documents", which is the finding that changes a decision.

Code is MIT. The data carries separate terms covering research, evaluation,
auditing, education and defensive privacy work. The paper and the repository
describe the data licence differently, so this needs resolving before use.

### RedactionBench — the hard set

Only 200 documents across 11 domains, but manually annotated and mostly seeded
from real sources, which makes it complementary to controlled synthesis rather
than redundant with it. Its framing is contextual integrity: whether a given
disclosure constitutes a violation depends on who holds the information and why,
which entity recognition alone does not capture.

It reports 35 models, including GLiNER variants and production privacy filters, so
part of the shortlist has published numbers to sanity-check ours against. CC BY 4.0.

### presidio-research — the harness

Not a dataset: a template-based synthetic generator plus an evaluation framework.
Two things worth taking from it regardless of which corpus we score on:

- It scores with **F2 / F2.5**, weighting recall above precision, on the grounds
  that a miss leaks and a false positive merely over-redacts. That is the right
  default here and [§7](#7-metrics) adopts it.
- The generator is the natural tool for domain-specific templates once we know
  what our own traffic looks like.

## 3. Data we should not score on

[`ai4privacy/pii-masking-300k`](https://huggingface.co/datasets/ai4privacy/pii-masking-300k)
is the obvious large multilingual option — OpenPII-220k plus FinPII-80k, six
languages, localization across eight jurisdictions — and it should **not** be our
scoring set, for two independent reasons.

**Licensing.** It is open for academic purposes with commercial use by separate
arrangement. That alone makes it unsuitable for a gate on shipped software.

**Provenance overlap, which is the more serious one.** It is the training
distribution for several shortlist candidates. Scoring `piiranha-v1` against
ai4privacy-derived data approximates testing on train, and the resulting number
would flatter it specifically.

This is not hypothetical. [*Unmasking the Reality of PII Masking
Models*](https://arxiv.org/abs/2504.12308) evaluates Piiranha and Starpii — both
widely downloaded — on 17,000 semi-synthetic sentences covering 16 PII types, along
six dimensions: basic entity recognition, contextual disambiguation, noisy data,
novel entity detection, cross-lingual capability and adversarial context. Both
degrade substantially outside their training distribution.

`piiranha-v1` is on our shortlist. **A benchmark that cannot reproduce that
finding is not measuring the thing we care about**, and one sharing provenance
with the training data cannot.

## 4. The taxonomy problem

This is where the effort actually goes. It is not data collection.

The seven models do not share a label inventory:

| Model | Label interface |
|---|---|
| `minilm-l6-pii` | own BIO tag set |
| `piiranha-v1` | 18 classes, fixed in the graph |
| `multilang-pii-ner` | 35 classes, fixed in the graph |
| `kiji-pii` | 53 classes plus a coreference head |
| `gliner-pii-edge`, `gliner-pii-base` | zero-shot, labels supplied per request |
| `gliner2-pii` | zero-shot, schema supplied per request |

Scoring these against one gold standard requires a mapping from each model's
labels into a canonical taxonomy, and **most of the measurement error will live in
that mapping rather than in the models**. Mappings are lossy in both directions: a
model with a single `ID_NUMBER` class cannot be credited or faulted for
distinguishing a passport number from a tax number, and one with fifteen
identifier subtypes will be punished by a coarse mapping for being precise.

Adopt REDACT's 51 canonical types rather than inventing a taxonomy. Record each
per-model mapping as data, reviewed independently of the code that consumes it,
and report any gold type that a given model's inventory cannot express — scoring a
model on categories it has no way to emit produces a number that says nothing.

**The zero-shot models need care in both directions.** Being able to hand GLiNER
the canonical labels directly removes the mapping error, which is a real
advantage. But it introduces prompt sensitivity as a variable, and it has a
performance consequence: `gliner-pii-edge` and `gliner-pii-base` were benchmarked
at **8 labels**, and their cost scales with the label count. A quality comparison
at 51 labels implies materially worse throughput than
[ADR 0009](../ADR/0009-cpt-cf-onnx-runner-adr-replica-pool-sizing.md) records.
**Quality and throughput must be measured at the same label count**, or the
comparison is not honest. Whichever count is chosen, both tables have to move to
it.

## 5. Negative controls

The measurement most likely to be omitted, and the most likely to change the
decision.

Every suite above scores recall and precision on text that *contains* PII. None of
it establishes what a model does with text that contains none. A model that flags
every capitalized token scores excellently on recall and is useless as a redactor,
because over-redaction destroys the document it was meant to make safe.

So: a corpus of text with **no PII at all**, scored on false positives per
thousand tokens. Public-domain literary text is well suited to this — it is the one
job where its distribution mismatch does not matter, since the correct output is
empty regardless of genre or century.

Report this as its own figure. Do not fold it into an F-score, where a large
false-positive rate on clean text can hide behind good recall on dirty text.

## 6. Injecting PII into public-domain text

Workable, and the weakest option for the quality set. Not recommended as primary.

These models classify from **context**, and injection perturbs context in ways
that flatter or punish arbitrarily:

- **Distribution mismatch.** A modern IBAN in nineteenth-century prose is
  detectable by surprise alone. The measurement becomes "can the model spot the
  anachronism", not "can it recognize an IBAN".
- **Grammatical placement.** Spans inserted without templates land in positions no
  writer would produce, which is neither an easy case nor a realistic hard one.
- **Locale incoherence.** A German name with a US phone format in a French
  sentence is not a difficult case; it is an incoherent one, and a model penalized
  for it has been mismeasured.

Where the technique is right:

- **Negative controls** ([§5](#5-negative-controls)) — no injection at all, and the
  mismatch is irrelevant.
- **The timing replay** ([§1](#1-two-questions-two-kinds-of-data)) — real token
  distributions, labels not needed.
- **Domain templates later**, via presidio-research's generator with locale-consistent
  providers, once our own traffic shape is known. Template-driven generation avoids
  the placement problem; ad-hoc sprinkling does not.

For carrier text prefer modern, permissive, genuinely multilingual sources —
EuroParl and JRC-Acquis (aligned EU documents), Wikipedia (CC BY-SA), Tatoeba,
OPUS — over Project Gutenberg, whose English is archaic. Gutenberg remains a good
choice for negative controls specifically.

## 7. Metrics

- **F2**, following presidio-research, weighting recall above precision: a missed
  entity leaks, a false positive over-redacts.
- **Per-type recall**, not only aggregates. An aggregate hides the case where a
  model is strong on names and blind to national identifiers, which for a
  redaction gate is the difference between usable and not.
- **Span matching reported at all three strictnesses** REDACT supports — exact,
  partial, fuzzy. Partial credit is legitimate for redaction, where covering most
  of an entity may still leak the rest; stating which mode a number came from
  matters more than choosing one.
- **False positives per thousand tokens** on clean text, reported separately
  ([§5](#5-negative-controls)).
- **Per-language breakdown.** `minilm-l6-pii` is English-only; a multilingual
  aggregate would quietly average that away, and it is currently the throughput
  leader.

## 7a. What to measure through

Score **`onnx-scanner`'s `PiiBackend::detect_entities`**, not a decoder written
for the benchmark.

That plugin already exists at `gears/prompt-scanner/plugins/onnx-scanner`, and
its output type is the one a scorer needs:

```rust
struct Entity { category: String, start: u32, end: u32, confidence: f32 }
```

Byte offsets into the scanned text, already merged into spans, with a
confidence. Three consequences:

- **It measures what ships.** A bespoke decoder would produce numbers for code
  no user ever runs; every BIO merging rule and span-assembly choice is part of
  detection quality, not incidental to it.
- **`confidence` makes threshold sweeps possible**, so the output is a
  precision/recall curve rather than a single point. The right threshold is a
  policy decision, and a single F-score hides it.
- **Backend coverage becomes the schedule.** Whatever has no backend cannot be
  scored.

Coverage today, against the seven measured models:

| Model | Backend | Scoreable now |
|---|---|---|
| `minilm-l6-pii` | `BioTokenClassifier` | yes |
| `piiranha-v1` | `BioTokenClassifier` | yes |
| `multilang-pii-ner` | `BioTokenClassifier` | yes |
| `kiji-pii` | `BioTokenClassifier` | probably; it also emits coreference logits |
| `gliner2-pii` | `Gliner2` | yes |
| `gliner-pii-edge` | **none** | no |
| `gliner-pii-base` | **none** | no |

**The two GLiNER v1 models have no backend**, and their contract differs from
GLiNER2's — `words_mask`, `text_lengths`, `span_idx`, `span_mask` rather than a
schema-indexed prompt. `gliner-pii-edge` is the leading candidate on size and
licence, so this is on the critical path, not a nice-to-have.

`BioTokenClassifier` takes its label set from plugin configuration. Now that
`list_models` serves `LoadedModelInfo.labels`, that duplication can go — one
fewer per-model thing to get right when adding models to an evaluation, and one
fewer place for a label set to drift from the graph it describes.

## 8. An anecdote, recorded as such

`gears/onnx-runner/onnx-runner/examples/pii_scan.rs` runs the whole path on one
English sentence containing a name, an email and a phone number. On
2026-09-01: `minilm-l6-pii` found all four spans exactly; `piiranha-v1` found
only the phone number, predicting `O` across the name and every token of the
email; `multilang-pii-ner` tagged one of two name tokens, two sub-tokens of the
email and none of the phone digits.

**One sentence is not a measurement** and must not move the model decision. It
is here because it is consistent with the published finding in
[§3](#3-data-we-should-not-score-on) about degradation outside the training
distribution, and because it establishes that the pipeline works end to end —
which is what makes the sentence evidence about the models rather than about
our code.

## 9. References

- REDACT: [paper](https://arxiv.org/abs/2606.19881) ·
  [repository](https://github.com/guneeshvats/REDACT-PII-Benchmark)
- RedactionBench: [paper](https://arxiv.org/abs/2606.18782)
- presidio-research: [repository](https://github.com/microsoft/presidio-research) ·
  [evaluation docs](https://microsoft.github.io/presidio/evaluation/)
- *Unmasking the Reality of PII Masking Models*:
  [paper](https://arxiv.org/abs/2504.12308)
- `ai4privacy/pii-masking-300k`:
  [dataset card](https://huggingface.co/datasets/ai4privacy/pii-masking-300k)
