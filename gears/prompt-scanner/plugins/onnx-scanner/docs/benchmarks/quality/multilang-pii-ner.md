corpus   REDACT: 1000 records, 17584 gold spans kept, 89 misannotated upstream
         6526 spans in 32 type(s) excluded as out of scope
         984 labelled, 0 negative controls
model    multilang-pii-ner (35 labels)
note     this model has no label for 2 in-scope categor(ies), scored as misses: IP, ORG

## multilang-pii-ner — detection quality

> 14063 predictions excluded as out of scope for this comparison, not counted against the model: OUT_OF_SCOPE (13846), TITLE (217)

> **5267 gold spans (30.0%) lie beyond this model's context window** and were truncated away before inference. They are excluded from recall; a deployment would still miss them.

| Match | Precision | Recall | F2 | TP | FP | FN |
|---|---|---|---|---|---|---|
| exact | 0.184 | 0.201 | 0.198 | 2481 | 10983 | 9836 |
| overlap | 0.498 | 0.544 | 0.534 | 6699 | 6765 | 5618 |

### Per category (overlap)

| Category | Recall | Precision | TP | FP | FN |
|---|---|---|---|---|---|
| ADDRESS | 0.808 | 0.316 | 1107 | 2399 | 263 |
| CREDIT_CARD | 0.101 | 0.141 | 10 | 61 | 89 |
| DOB | 0.000 | 0.000 | 0 | 0 | 491 |
| EMAIL | 0.896 | 0.796 | 908 | 233 | 105 |
| ID | 0.890 | 0.171 | 479 | 2316 | 59 |
| IP | 0.000 | 0.000 | 0 | 0 | 59 |
| ORG | 0.000 | 0.000 | 0 | 0 | 676 |
| PERSON | 0.493 | 0.808 | 3621 | 861 | 3723 |
| PHONE | 0.966 | 0.391 | 574 | 895 | 20 |
| USERNAME | 0.000 | 0.000 | 0 | 0 | 133 |

### Per language (overlap)

| Language | Recall | F2 |
|---|---|---|
| AR | 0.615 | 0.601 |
| CS | 0.484 | 0.469 |
| DA | 0.531 | 0.528 |
| DE | 0.547 | 0.541 |
| EN | 0.529 | 0.506 |
| ES | 0.554 | 0.540 |
| FI | 0.582 | 0.563 |
| FR | 0.493 | 0.505 |
| FR_CA | 0.537 | 0.526 |
| HE | 0.520 | 0.518 |
| HI | 0.477 | 0.482 |
| HU | 0.491 | 0.497 |
| IT | 0.547 | 0.544 |
| JA | 0.536 | 0.520 |
| KO | 0.688 | 0.654 |
| NL | 0.479 | 0.480 |
| NO | 0.511 | 0.492 |
| PT_BR | 0.555 | 0.547 |
| PT_EU | 0.575 | 0.553 |
| RU | 0.580 | 0.563 |
| SV | 0.547 | 0.540 |
| TH | 0.544 | 0.519 |
| TR | 0.474 | 0.479 |
| ZH_CN | 0.666 | 0.623 |
| ZH_TW | 0.609 | 0.590 |
