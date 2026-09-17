#!/usr/bin/env bash
# Download the ONNX Runtime shared library, by pinned version, with sha256
# verification.
#
# Repo-level rather than per-gear because two gears need it:
#
#   - file-parser, behind its `magika` feature (content-type detection)
#   - onnx-runner, always (it is the gear's whole job)
#
# Both reach ONNX Runtime through the workspace `ort` dependency, which is
# pinned with the `load-dynamic` feature (see the `ort` entry in the root
# Cargo.toml). That means the library is `dlopen`ed from `ORT_DYLIB_PATH` at
# process start rather than linked at build time. Get it wrong — unset,
# missing, or older than the pinned `api-24` ABI — and `ort` 2.0.0-rc.12
# **hangs instead of erroring**. Both gears bound that with a timeout at
# startup; this script is how you avoid hitting it at all.
#
# Local dev / CI / testing convenience only, like fetch-models.sh: production
# operators provision the runtime themselves.
#
# Usage:
#   bash scripts/fetch-onnxruntime.sh              # fetch (idempotent)
#   bash scripts/fetch-onnxruntime.sh --path       # print the path, for export
#
#   export ORT_DYLIB_PATH="$(bash scripts/fetch-onnxruntime.sh --path)"

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUNTIME_DIR="${ONNXRUNTIME_DIR:-$REPO_ROOT/.onnxruntime}"

# Must satisfy the workspace `ort` pin's `api-24` feature, i.e. ONNX Runtime
# >= 1.24; older builds fail `ort`'s version check on load. Bump alongside the
# `ort` pin — and update every sha256 below when you do.
VERSION="${ONNXRUNTIME_VERSION:-1.29.0}"

# sha256 of the release tarball, per platform. Same artifact and pinning style
# the `test-magika` CI job used inline before this script existed.
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)
    PLATFORM="osx-arm64"; LIBNAME="libonnxruntime.$VERSION.dylib"; LINKNAME="libonnxruntime.dylib"
    SHA256="d0706fc34f315d8c88639d0a8c81f2e09e815f282cabed3493c06a054352cf92" ;;
  Darwin-x86_64)
    PLATFORM="osx-x86_64"; LIBNAME="libonnxruntime.$VERSION.dylib"; LINKNAME="libonnxruntime.dylib"
    SHA256="" ;;
  Linux-x86_64)
    PLATFORM="linux-x64"; LIBNAME="libonnxruntime.so.$VERSION"; LINKNAME="libonnxruntime.so"
    SHA256="c3fddc4f139a045b0c4902c57410f0694f1c2fdf9b6939fbe38b1aeae7cd14ba" ;;
  Linux-aarch64)
    PLATFORM="linux-aarch64"; LIBNAME="libonnxruntime.so.$VERSION"; LINKNAME="libonnxruntime.so"
    SHA256="" ;;
  *)
    echo "unsupported platform: $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac

TARBALL="onnxruntime-$PLATFORM-$VERSION.tgz"
URL="https://github.com/microsoft/onnxruntime/releases/download/v$VERSION/$TARBALL"
OUT="$RUNTIME_DIR/$LIBNAME"

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

if [[ "${1:-}" == "--path" ]]; then
  # Resolve-only mode. Fetches first if needed, so a caller can write
  #   export ORT_DYLIB_PATH="$(bash scripts/fetch-onnxruntime.sh --path)"
  # without a separate setup step. Progress goes to stderr so stdout stays
  # exactly the path.
  [[ -f "$RUNTIME_DIR/$LINKNAME" ]] || bash "${BASH_SOURCE[0]}" >&2
  echo "$RUNTIME_DIR/$LINKNAME"
  exit 0
fi

if [[ -f "$OUT" ]]; then
  echo "ok       $LIBNAME (cached)"
else
  mkdir -p "$RUNTIME_DIR"
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT

  echo "fetch    $TARBALL  <-  microsoft/onnxruntime v$VERSION"
  curl -fSL --retry 3 --retry-delay 2 -o "$tmp/$TARBALL" "$URL"

  if [[ -n "$SHA256" ]]; then
    got="$(sha256_of "$tmp/$TARBALL")"
    if [[ "$got" != "$SHA256" ]]; then
      echo "FAIL     $TARBALL sha256 mismatch" >&2
      echo "         expected $SHA256" >&2
      echo "         actual   $got" >&2
      exit 1
    fi
    echo "verified $TARBALL"
  else
    echo "WARN     no pinned checksum for $PLATFORM — fetched unverified." >&2
    echo "         Add this to the script once confirmed: $(sha256_of "$tmp/$TARBALL")" >&2
  fi

  tar -xzf "$tmp/$TARBALL" -C "$tmp"
  src="$tmp/onnxruntime-$PLATFORM-$VERSION/lib/$LIBNAME"
  [[ -f "$src" ]] || { echo "FAIL     $LIBNAME not found in $TARBALL" >&2; exit 1; }
  cp "$src" "$OUT"
fi

ln -sf "$LIBNAME" "$RUNTIME_DIR/$LINKNAME"

echo
echo "ORT_DYLIB_PATH=$RUNTIME_DIR/$LINKNAME"
