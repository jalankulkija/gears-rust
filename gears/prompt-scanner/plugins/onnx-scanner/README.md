# onnx-scanner

`prompt-scanner` scanner-backend plugin that runs PII detection through `onnx-runner`.
Two decoder families behind one `ScannerBackendClient`: GLiNER2 span extraction
(`PiiBackendConfig::Gliner2`) and BIO token classification
(`PiiBackendConfig::BioTokenClassifier`), selected by config.

It owns the model-specific tensor work — prompt construction, span decoding, word
alignment, label mapping — and nothing about sessions or model lifecycle, which belong to
`onnx-runner`.

## Provenance

The GLiNER2 decode path (`src/domain/gliner2/{mod,decode,prompt,tensor,words}.rs`) and the
`GLINER2_PII` label schema are derived from **dlp-bench**, a public Rust benchmark for
local PII-detection models:

> <https://github.com/MikeFalcon77/dlp-bench> — MIT, Copyright (c) 2026 MikeY

Per-module doc comments name the specific upstream files they came from. This notice is
the attribution MIT asks for; see that repository's `LICENSE` for the full terms.
