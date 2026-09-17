Created:  2026-09-01 by Constructor Tech
Updated:  2026-09-01 by Constructor Tech

# Detection quality — measured

Results for the method designed in
[QUALITY-EVALUATION.md](./QUALITY-EVALUATION.md). Throughput for the same
models is in [ADR 0009](../ADR/0009-cpt-cf-onnx-runner-adr-replica-pool-sizing.md); neither number means much without the
other.

**Do not read the headline F2 alone.** Two of the four models never saw a third
of the gold spans, and every model was asked only about the categories at least
one of them supports. [§5](#5-how-to-read-this) is not optional context.

<!-- toc -->

- [1. Results](#1-results)
- [2. Per category](#2-per-category)
- [3. Per language](#3-per-language)
- [4. What this changes](#4-what-this-changes)
- [5. How to read this](#5-how-to-read-this)
- [6. Not measured](#6-not-measured)
- [7. Reproducing](#7-reproducing)

<!-- /toc -->

## 1. Results

REDACT locked 1000-record split, 25 languages, 984 scored documents, 17,584 gold
spans. Overlap matching, F2 (recall-weighted). Run 2026-09-01.

| Model | Precision | Recall | **F2** | Exact F2 | Truncated gold |
|---|---|---|---|---|---|
| `gliner2-pii` *(current)* | **0.696** | 0.506 | **0.536** | **0.499** | none |
| `multilang-pii-ner` | 0.498 | **0.544** | 0.534 | 0.198 | 30.0% |
| `minilm-l6-pii` | 0.420 | 0.376 | 0.384 | 0.240 | 39.2% |
| `piiranha-v1` | 0.643 | 0.069 | 0.084 | 0.005 | none |

Raw output per model is in [`quality/`](./quality/).

**The model we ship is the most accurate one tested.** That was not the expected
result: `gliner2-pii` is also the slowest by a factor of twelve, and the
performance work had framed it as the thing to replace.

**`multilang-pii-ner` ties it on F2 while being 5.3x faster** — and does so
despite never seeing 30% of the gold spans. It is the strongest candidate on the
combined picture, and the one to look at next.

**Exact-match F2 separates the field far more than overlap does.** `gliner2-pii`
loses 7% going from overlap to exact; `multilang-pii-ner` loses 63%. Its spans
overlap the right text but rarely land on the right boundaries, which matters
for redaction: a span that covers most of an account number still leaks the
rest.

**`piiranha-v1` should be dropped from consideration.** Recall 0.069 with no
truncation to excuse it — it finds 1,213 of 17,584 spans. Precision 0.643 says
it is usually right when it speaks, and almost always silent. For a redaction
gate that is the worst available failure mode. This is consistent with the
published finding on degradation outside the training distribution
([QUALITY-EVALUATION.md §3](./QUALITY-EVALUATION.md#3-data-we-should-not-score-on)).

## 2. Per category

Recall, overlap matching. `-` means the model has no label for the category and
scores zero by construction.

| Category | `gliner2-pii` | `multilang` | `minilm` | `piiranha` |
|---|---|---|---|---|
| EMAIL | **0.974** | 0.896 | **0.990** | 0.012 |
| PHONE | **0.963** | **0.966** | 0.707 | 0.509 |
| DOB | **0.974** | - | 0.176 | 0.082 |
| IP | **0.903** | - | 0.885 | - |
| ORG | **0.878** | - | 0.165 | - |
| USERNAME | **0.834** | - | 0.341 | 0.083 |
| ID | 0.752 | **0.890** | 0.086 | 0.050 |
| CREDIT_CARD | **0.553** | 0.101 | 0.035 | - |
| PERSON | 0.376 | **0.493** | 0.295 | 0.017 |
| ADDRESS | 0.159 | **0.808** | 0.519 | 0.241 |

**No model is good at names**, and names are the largest category in the corpus
by a wide margin — 10,615 gold `PERSON` spans, more than a third of everything
scored. The best recall is 0.493. Whatever ships will miss names, so a policy
that depends on catching them needs a second mechanism.

**`gliner2-pii` and `multilang-pii-ner` fail in opposite directions.** GLiNER2 is
strong on structured identifiers and weak on addresses (0.159); `multilang` is
the reverse (0.808 on addresses, 0.101 on card numbers). That is an argument for
looking at whether the two are complementary rather than picking one.

**Precision varies more than recall.** `gliner2-pii` reaches 0.929 precision on
`PERSON` but 0.129 on `USERNAME` — it flags 1,022 spurious usernames. Per-model
category thresholds would be worth exploring before any switch.

## 3. Per language

`gliner2-pii` across all 25, recall on overlap matching:

| Best | | Worst | |
|---|---|---|---|
| KO | 0.668 | DA | 0.422 |
| TH | 0.632 | PT_BR | 0.436 |
| AR | 0.589 | IT | 0.452 |
| HE | 0.578 | TR | 0.455 |
| PT_EU | 0.563 | FR | 0.470 |

**The spread is 0.42 to 0.67 with English at 0.529 — mid-table.** Nothing here
suggests a European-language bias, and the non-Latin scripts (Korean, Thai,
Arabic, Hebrew) are at the top rather than the bottom. That is a better result
for a multilingual deployment than expected, and it is the axis a single-language
smoke test cannot see at all.

## 4. What this changes

**The model swap is not obviously worth making.** The performance work found a
12x throughput spread and recommended moving off `gliner2-pii`. On quality it is
the best of the four. The honest summary is that `gliner2-pii` buys accuracy with
throughput, and `multilang-pii-ner` offers roughly the same F2 for 5.3x the
speed — but with far worse span boundaries and a 512-token window.

**Context window is a deployment constraint, not a footnote.** `minilm-l6-pii`
cannot see 39% of REDACT's gold spans and `multilang-pii-ner` 30%, because their
tokenizers truncate at 512 tokens. Those spans are excluded from the recall
above; **a deployment would still miss them**. Either model needs input chunking
before it could be used on documents of this length, and that chunking is work
neither the gear nor the scanner does today.

**`gliner-pii-edge` remains unmeasured and remains the interesting candidate.**
44 MB, Apache-2.0, 5.7x the throughput, zero-shot spans retained. It has no
backend in `onnx-scanner`, so it is absent from every table here. Deciding by
default is the outcome to avoid — see [§6](#6-not-measured).

## 5. How to read this

Every number above is conditioned on choices recorded in
[QUALITY-EVALUATION.md](./QUALITY-EVALUATION.md). The ones that move results
most:

- **The gold taxonomy collapses REDACT's 51 types into 24 and excludes 32**
  (6,526 spans) that no model claims. Those are out of scope, not misses.
  `eval-corpora/taxonomy/gold-redact.txt` is where that judgement lives and is
  the largest remaining source of measurement error.
- **Predictions in categories outside the gold scope are excluded, not counted
  as false positives.** `minilm-l6-pii` emits 4,401 `DATE` spans; the comparison
  does not score dates. Counting them would punish a model for detecting more
  than it was asked.
- **Gold beyond a model's context window is excluded from recall.** Otherwise
  truncation is indistinguishable from poor detection.
- **Adjacent same-category predictions are merged before matching**, so a model
  emitting `first_name` + `last_name` over one gold `PERSON` is not penalised for
  being more specific than the annotation.
- **89 gold annotations are skipped** — REDACT marks them `string_not_in_text`
  and gives them null offsets. Upstream defects, not scored against anyone.
- **F2 weights recall twice as heavily as precision**, following
  presidio-research: a missed entity leaks, a false positive over-redacts.

## 6. Not measured

- **`gliner-pii-edge` and `gliner-pii-base`** — no backend. GLiNER v1's contract
  differs from GLiNER2's, and the two variants need different decoders: `edge` is
  token-level, `base` scores explicit candidate spans.
- **`kiji-pii`** — a `BioTokenClassifier` should drive it, but it also emits
  coreference logits and has not been run.
- **Negative controls on this corpus.** REDACT has no PII-free documents; the
  false-positive-on-clean-text figures come from the smoke corpus only, where all
  models scored zero unambiguous false positives on 1,010 characters. That is far
  too little text to conclude anything.
- **Confidence thresholds.** Everything is at each backend's default. The
  `Entity.confidence` field makes a precision/recall sweep possible and it has
  not been run — likely the cheapest remaining improvement, given how much
  precision varies by category.
- **The label-count coupling.** `gliner2-pii` was scored with 13 runtime labels.
  GLiNER cost is `spans x labels`, so its throughput figures must be taken at the
  same count to be comparable. They currently are not.

## 7. Reproducing

```sh
export ORT_DYLIB_PATH="$(bash scripts/fetch-onnxruntime.sh --path)"
bash gears/prompt-scanner/scripts/fetch-eval-corpora.sh
bash gears/onnx-runner/scripts/fetch-models.sh          # all models

D=gears/prompt-scanner/plugins/onnx-scanner
cargo run --release -p cf-gears-onnx-scanner --features eval --bin evaluate -- \
  --model multilang-pii-ner --redact \
  --corpus $D/eval-corpora/redact-sample1000.json \
  --gold-taxonomy-file $D/eval-corpora/taxonomy/gold-redact.txt \
  --taxonomy-file $D/eval-corpora/taxonomy/multilang-pii-ner.txt

# The zero-shot backend takes its labels at request time.
cargo run --release -p cf-gears-onnx-scanner --features eval --bin evaluate -- \
  --model gliner2-pii --gliner2 --redact ...
```

The corpus is not committed. It is fetched at a pinned revision with sha256
verification; REDACT is synthetic benchmark data under the
[REDACT Dataset Terms](https://github.com/guneeshvats/REDACT-PII-Benchmark/blob/252232ac29669beeb99cb6474eeb2736761be508/DATASET_TERMS.md),
which permit benchmarking and defensive privacy work.

Cite REDACT as: Vats et al., "REDACT: A Systematically Controlled Multilingual
Benchmark for Personal Information Detection", EMNLP 2026 Industry Track
([preprint](https://arxiv.org/abs/2606.19881)).
