#!/usr/bin/env bash
# Download onnx-runner model artifacts by pinned revision, with sha256
# verification. Local dev / CI / testing convenience only — see
# gears/onnx-runner/docs/DESIGN.md's "Model provisioning" decision:
# `onnx-runner` itself never fetches anything; it only loads and verifies
# what's already on disk. Production operators place model files themselves.
#
# Same pinned-revision + per-file-checksum pattern as dlp-bench's fetch script
# (https://github.com/MikeFalcon77/dlp-bench, MIT).
#
# Usage: bash gears/onnx-runner/scripts/fetch-models.sh [model-id ...]  (default: all)

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODELS_DIR="${ONNX_RUNNER_MODELS_DIR:-$ROOT/models}"

# ---------------------------------------------------------------------------
# Manifest: <local-dir>|<hf-repo>|<revision>|<remote-path>|<local-name>|<sha256>|<bytes>
# Revisions are immutable commit SHAs. `main` is never accepted.
# An empty sha256 means "fetch but don't verify" — used for small config
# files that aren't worth pinning a checksum for.
# ---------------------------------------------------------------------------
MANIFEST=(
  # minilm-l6-pii: gears/onnx-runner/docs/DESIGN.md's original "Which PII
  # model" decision — an explicit placeholder (English-only), superseded by
  # gliner2-pii-onnx below as onnx-scanner's default backend, but kept
  # fetchable since `PiiBackendConfig::BioTokenClassifier` can still select
  # it. Same artifact dlp-bench validated at this revision.
  "minilm-l6-pii|Negative-Star-Innovators/MiniLM-L6-finetuned-pii-detection-v2|80a05dbac61de613ea01aee8542b465bb51c54b2|onnx/model.onnx|model.onnx|8341930b1c41fb05ffffc737c334cb78ff073c161a7806654a944d7355be893b|90542099"
  "minilm-l6-pii|Negative-Star-Innovators/MiniLM-L6-finetuned-pii-detection-v2|80a05dbac61de613ea01aee8542b465bb51c54b2|tokenizer.json|tokenizer.json||"
  "minilm-l6-pii|Negative-Star-Innovators/MiniLM-L6-finetuned-pii-detection-v2|80a05dbac61de613ea01aee8542b465bb51c54b2|config.json|config.json||"
  "minilm-l6-pii|Negative-Star-Innovators/MiniLM-L6-finetuned-pii-detection-v2|80a05dbac61de613ea01aee8542b465bb51c54b2|tokenizer_config.json|tokenizer_config.json||"

  # gliner2-pii-onnx: Fastino's GLiNER2 PII model, community ONNX export —
  # onnx-scanner's default backend (PiiBackendConfig::Gliner2). Same
  # artifact + revision dlp-bench validated, which also documented the
  # provenance caveats for this community export.
  "gliner2-pii|yethdev/gliner2-pii-onnx|7d82c70220c2b0891d4bda612402602b63be1c61|model.quantized.onnx|model.quantized.onnx|489edc522a34b8c6a0630c4710998abf0227b11acf45600ba2bce95bcd943930|446391629"
  "gliner2-pii|yethdev/gliner2-pii-onnx|7d82c70220c2b0891d4bda612402602b63be1c61|tokenizer.json|tokenizer.json|f6df10ec83bea993035b2dd7c39345a3d4fcf23421c2adb6cb4ffc1e6d1bc4b5|16020604"
  "gliner2-pii|yethdev/gliner2-pii-onnx|7d82c70220c2b0891d4bda612402602b63be1c61|tokenizer_config.json|tokenizer_config.json|233beed1f1095cccfc7907cde31a8d90a0c6aa4fdfaf6493f8e55fd162e81ae6|711"

  # ---- piiranha-v1 ----------------------------------------------------------
  # A BIO token classifier, used by `examples/pii_scan.rs` to show what a
  # consumer of this gear looks like end to end. Not wired into any shipped
  # config: the example is the only thing that fetches it. mDeBERTa-v3; the
  # int8 and "quantized" exports in this repo are byte-identical, so the int8
  # name is pinned.
"piiranha-v1|onnx-community/piiranha-v1-detect-personal-information-ONNX|888e40cc88efa70f39a47ce2d5ea789b5e997278|onnx/model_int8.onnx|model_int8.onnx|d5f4d139371b9eeab687d705604e928c46a28a8169654323888bb3160e839076|317144829"
  "piiranha-v1|onnx-community/piiranha-v1-detect-personal-information-ONNX|888e40cc88efa70f39a47ce2d5ea789b5e997278|tokenizer.json|tokenizer.json|8d5a786a62a72797fcd2f23663c3f8665bc81e6f10043004c66490810b3d0d82|16335516"
  "piiranha-v1|onnx-community/piiranha-v1-detect-personal-information-ONNX|888e40cc88efa70f39a47ce2d5ea789b5e997278|tokenizer_config.json|tokenizer_config.json||"
  "piiranha-v1|onnx-community/piiranha-v1-detect-personal-information-ONNX|888e40cc88efa70f39a47ce2d5ea789b5e997278|config.json|config.json||"
)

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

want="${1:-}"
downloaded=0
verified=0

for row in "${MANIFEST[@]}"; do
  IFS='|' read -r dir repo rev remote name sha bytes <<<"$row"
  [[ -n "$want" && "$want" != "$dir" ]] && continue

  case "$rev" in
    main|master|HEAD) echo "refusing unpinned revision '$rev' for $repo" >&2; exit 1 ;;
  esac

  dest="$MODELS_DIR/$dir"
  out="$dest/$name"
  mkdir -p "$dest"

  if [[ -f "$out" && -n "$sha" ]]; then
    if [[ "$(sha256_of "$out")" == "$sha" ]]; then
      echo "ok       $dir/$name (cached)"
      verified=$((verified + 1))
      continue
    fi
    echo "stale    $dir/$name — checksum mismatch, re-downloading" >&2
    rm -f "$out"
  elif [[ -f "$out" && -z "$sha" ]]; then
    echo "ok       $dir/$name (cached, unchecked)"
    continue
  fi

  url="https://huggingface.co/$repo/resolve/$rev/$remote"
  echo "fetch    $dir/$name  <-  $repo@${rev:0:8}"
  curl -fSL --retry 3 --retry-delay 2 -o "$out.part" "$url"
  mv "$out.part" "$out"
  downloaded=$((downloaded + 1))

  if [[ -n "$sha" ]]; then
    got="$(sha256_of "$out")"
    if [[ "$got" != "$sha" ]]; then
      echo "FAIL     $dir/$name sha256 mismatch" >&2
      echo "         expected $sha" >&2
      echo "         actual   $got" >&2
      rm -f "$out"
      exit 1
    fi
    actual_bytes="$(wc -c <"$out" | tr -d ' ')"
    if [[ -n "$bytes" && "$actual_bytes" != "$bytes" ]]; then
      echo "FAIL     $dir/$name size mismatch: expected $bytes, got $actual_bytes" >&2
      rm -f "$out"
      exit 1
    fi
    echo "verified $dir/$name"
    verified=$((verified + 1))
  fi
done

echo
echo "downloaded=$downloaded verified=$verified  ->  $MODELS_DIR"
