corpus   REDACT: 1000 records, 17584 gold spans kept, 89 misannotated upstream
         6526 spans in 32 type(s) excluded as out of scope
         984 labelled, 0 negative controls
model    minilm-l6-pii (111 labels)

## minilm-l6-pii — detection quality

> 6378 predictions excluded as out of scope for this comparison, not counted against the model: ACCOUNT (101), AGE (7), DATE (4401), OCCUPATION (189), OUT_OF_SCOPE (1569), SECRET (31), URL (80)

> **6893 gold spans (39.2%) lie beyond this model's context window** and were truncated away before inference. They are excluded from recall; a deployment would still miss them.

| Match | Precision | Recall | F2 | TP | FP | FN |
|---|---|---|---|---|---|---|
| exact | 0.262 | 0.235 | 0.240 | 2510 | 7055 | 8181 |
| overlap | 0.420 | 0.376 | 0.384 | 4017 | 5548 | 6674 |

### Per category (overlap)

| Category | Recall | Precision | TP | FP | FN |
|---|---|---|---|---|---|
| ADDRESS | 0.519 | 0.293 | 616 | 1489 | 570 |
| CREDIT_CARD | 0.035 | 0.044 | 3 | 65 | 83 |
| DOB | 0.176 | 0.888 | 79 | 10 | 370 |
| EMAIL | 0.990 | 0.756 | 866 | 279 | 9 |
| ID | 0.086 | 0.625 | 40 | 24 | 425 |
| IP | 0.885 | 0.319 | 46 | 98 | 6 |
| ORG | 0.165 | 0.146 | 96 | 562 | 485 |
| PERSON | 0.295 | 0.434 | 1882 | 2455 | 4501 |
| PHONE | 0.707 | 0.756 | 347 | 112 | 144 |
| USERNAME | 0.341 | 0.085 | 42 | 454 | 81 |

### Per language (overlap)

| Language | Recall | F2 |
|---|---|---|
| AR | 0.156 | 0.185 |
| CS | 0.410 | 0.375 |
| DA | 0.410 | 0.392 |
| DE | 0.434 | 0.445 |
| EN | 0.544 | 0.535 |
| ES | 0.444 | 0.466 |
| FI | 0.459 | 0.412 |
| FR | 0.473 | 0.491 |
| FR_CA | 0.570 | 0.558 |
| HE | 0.198 | 0.231 |
| HI | 0.347 | 0.381 |
| HU | 0.380 | 0.352 |
| IT | 0.428 | 0.441 |
| JA | 0.150 | 0.178 |
| KO | 0.226 | 0.262 |
| NL | 0.425 | 0.419 |
| NO | 0.466 | 0.419 |
| PT_BR | 0.449 | 0.459 |
| PT_EU | 0.444 | 0.452 |
| RU | 0.106 | 0.125 |
| SV | 0.531 | 0.481 |
| TH | 0.213 | 0.246 |
| TR | 0.346 | 0.317 |
| ZH_CN | 0.181 | 0.213 |
| ZH_TW | 0.241 | 0.281 |
