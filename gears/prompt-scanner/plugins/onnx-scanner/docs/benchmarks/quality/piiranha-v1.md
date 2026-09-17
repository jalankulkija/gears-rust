corpus   REDACT: 1000 records, 17584 gold spans kept, 89 misannotated upstream
         6526 spans in 32 type(s) excluded as out of scope
         984 labelled, 0 negative controls
model    piiranha-v1 (18 labels)
note     this model has no label for 2 in-scope categor(ies), scored as misses: IP, ORG

## piiranha-v1 — detection quality

> 35 predictions excluded as out of scope for this comparison, not counted against the model: ACCOUNT (35)

| Match | Precision | Recall | F2 | TP | FP | FN |
|---|---|---|---|---|---|---|
| exact | 0.040 | 0.004 | 0.005 | 76 | 1810 | 17508 |
| overlap | 0.643 | 0.069 | 0.084 | 1213 | 673 | 16371 |

### Per category (overlap)

| Category | Recall | Precision | TP | FP | FN |
|---|---|---|---|---|---|
| ADDRESS | 0.241 | 0.509 | 448 | 433 | 1413 |
| CREDIT_CARD | 0.000 | 0.000 | 0 | 0 | 132 |
| DOB | 0.082 | 0.814 | 48 | 11 | 538 |
| EMAIL | 0.012 | 0.720 | 18 | 7 | 1470 |
| ID | 0.050 | 0.420 | 37 | 51 | 696 |
| IP | 0.000 | 0.000 | 0 | 0 | 103 |
| ORG | 0.000 | 0.000 | 0 | 0 | 962 |
| PERSON | 0.017 | 0.872 | 177 | 26 | 10438 |
| PHONE | 0.509 | 0.847 | 470 | 85 | 453 |
| USERNAME | 0.083 | 0.200 | 15 | 60 | 166 |

### Per language (overlap)

| Language | Recall | F2 |
|---|---|---|
| AR | 0.083 | 0.101 |
| CS | 0.049 | 0.060 |
| DA | 0.050 | 0.062 |
| DE | 0.075 | 0.092 |
| EN | 0.076 | 0.093 |
| ES | 0.069 | 0.084 |
| FI | 0.096 | 0.116 |
| FR | 0.065 | 0.079 |
| FR_CA | 0.068 | 0.083 |
| HE | 0.026 | 0.032 |
| HI | 0.065 | 0.080 |
| HU | 0.050 | 0.061 |
| IT | 0.078 | 0.095 |
| JA | 0.089 | 0.107 |
| KO | 0.073 | 0.088 |
| NL | 0.055 | 0.067 |
| NO | 0.084 | 0.102 |
| PT_BR | 0.055 | 0.067 |
| PT_EU | 0.076 | 0.093 |
| RU | 0.089 | 0.108 |
| SV | 0.054 | 0.066 |
| TH | 0.107 | 0.127 |
| TR | 0.032 | 0.040 |
| ZH_CN | 0.125 | 0.150 |
| ZH_TW | 0.090 | 0.108 |
