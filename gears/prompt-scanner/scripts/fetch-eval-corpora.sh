#!/usr/bin/env bash
# Downloads evaluation corpora by pinned revision, with sha256 verification.
#
# Same pattern as gears/onnx-runner/scripts/fetch-models.sh, and for the same
# reason: nothing is fetched at run time, and what is fetched is pinned.
#
# The corpora are NOT committed. They carry their own terms, they are large,
# and a benchmark whose inputs drift silently is worse than none.
#
# Usage: bash gears/prompt-scanner/scripts/fetch-eval-corpora.sh [name|all]

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="${EVAL_CORPORA_DIR:-$ROOT/plugins/onnx-scanner/eval-corpora}"

# REDACT: a systematically controlled multilingual benchmark for personal
# information detection (25 languages, 51 entity types). Code is MIT; the
# dataset carries REDACT Dataset Terms, which permit benchmarking and
# defensive PII-detection work and require the resource to be identified as
# REDACT and described as synthetic. We redistribute nothing.
#
#   https://github.com/guneeshvats/REDACT-PII-Benchmark
#   https://arxiv.org/abs/2606.19881
REPO="guneeshvats/REDACT-PII-Benchmark"
REV="252232ac29669beeb99cb6474eeb2736761be508"

# <name>|<path in repo>|<sha256>|<bytes>
# The sha256 is the file's git-lfs oid, which is its sha256 by construction.
MANIFEST=(
  "redact-sample1000|data/pii_benchmark_sample1000.json|754f562ce9c3c5b7b663f1ba6127c38a31d93f5631d38364341cb727936234c6|15931515"
  "redact-full|data/pii_benchmark_full.json|f8cb68bf5e54791decd9190792766b37e623bdb346b1641c1d1fd146ddec6d1c|213278031"
)

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

want="${1:-redact-sample1000}"
mkdir -p "$DEST"

for row in "${MANIFEST[@]}"; do
  IFS='|' read -r name path sha bytes <<<"$row"
  [[ "$want" != "all" && "$want" != "$name" ]] && continue

  out="$DEST/$name.json"
  if [[ -f "$out" && "$(sha256_of "$out")" == "$sha" ]]; then
    echo "ok       $name (cached)"
    continue
  fi

  # The media endpoint serves git-lfs content; a plain raw.githubusercontent
  # URL returns the 130-byte pointer file instead, which parses as neither
  # JSON nor an error.
  url="https://media.githubusercontent.com/media/$REPO/$REV/$path"
  echo "fetch    $name  <-  $REPO@${REV:0:8}"
  curl -fSL --retry 3 --retry-delay 2 -o "$out.part" "$url"
  mv "$out.part" "$out"

  got="$(sha256_of "$out")"
  if [[ "$got" != "$sha" ]]; then
    echo "FAIL     $name sha256 mismatch" >&2
    echo "         expected $sha" >&2
    echo "         actual   $got" >&2
    rm -f "$out"
    exit 1
  fi
  actual_bytes="$(wc -c <"$out" | tr -d ' ')"
  if [[ "$actual_bytes" != "$bytes" ]]; then
    echo "FAIL     $name size mismatch: expected $bytes, got $actual_bytes" >&2
    rm -f "$out"
    exit 1
  fi
  echo "verified $name"
done

echo
echo "-> $DEST"
echo "REDACT is synthetic benchmark data under the REDACT Dataset Terms:"
echo "   https://github.com/$REPO/blob/$REV/DATASET_TERMS.md"
