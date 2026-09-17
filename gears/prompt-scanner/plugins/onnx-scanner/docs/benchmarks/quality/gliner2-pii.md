corpus   REDACT: 1000 records, 17584 gold spans kept, 89 misannotated upstream
         6526 spans in 32 type(s) excluded as out of scope
         984 labelled, 0 negative controls
model    gliner2-pii (zero-shot)
labels   13 supplied at request time

## gliner2-pii — detection quality

> 193 predictions excluded as out of scope for this comparison, not counted against the model: ACCOUNT (193)

| Match | Precision | Recall | F2 | TP | FP | FN |
|---|---|---|---|---|---|---|
| exact | 0.648 | 0.472 | 0.499 | 8302 | 4500 | 9282 |
| overlap | 0.696 | 0.506 | 0.536 | 8906 | 3896 | 8678 |

### Per category (overlap)

| Category | Recall | Precision | TP | FP | FN |
|---|---|---|---|---|---|
| ADDRESS | 0.159 | 0.834 | 296 | 59 | 1565 |
| CREDIT_CARD | 0.553 | 0.689 | 73 | 33 | 59 |
| DOB | 0.974 | 0.832 | 571 | 115 | 15 |
| EMAIL | 0.974 | 0.958 | 1450 | 64 | 38 |
| ID | 0.752 | 0.697 | 551 | 240 | 182 |
| IP | 0.903 | 0.612 | 93 | 59 | 10 |
| ORG | 0.878 | 0.308 | 845 | 1902 | 117 |
| PERSON | 0.376 | 0.929 | 3987 | 305 | 6628 |
| PHONE | 0.963 | 0.902 | 889 | 97 | 34 |
| USERNAME | 0.834 | 0.129 | 151 | 1022 | 30 |

### Per language (overlap)

| Language | Recall | F2 |
|---|---|---|
| AR | 0.589 | 0.617 |
| CS | 0.465 | 0.491 |
| DA | 0.422 | 0.448 |
| DE | 0.492 | 0.528 |
| EN | 0.529 | 0.543 |
| ES | 0.490 | 0.520 |
| FI | 0.514 | 0.536 |
| FR | 0.470 | 0.504 |
| FR_CA | 0.489 | 0.523 |
| HE | 0.578 | 0.604 |
| HI | 0.477 | 0.501 |
| HU | 0.521 | 0.554 |
| IT | 0.452 | 0.489 |
| JA | 0.544 | 0.576 |
| KO | 0.668 | 0.668 |
| NL | 0.478 | 0.510 |
| NO | 0.522 | 0.546 |
| PT_BR | 0.436 | 0.478 |
| PT_EU | 0.563 | 0.585 |
| RU | 0.512 | 0.542 |
| SV | 0.538 | 0.563 |
| TH | 0.632 | 0.652 |
| TR | 0.455 | 0.488 |
| ZH_CN | 0.527 | 0.564 |
| ZH_TW | 0.472 | 0.501 |
