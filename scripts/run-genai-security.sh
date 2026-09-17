#!/usr/bin/env bash
# Runs the GenAI Security pipeline: cf-gears-example-server (forward-proxy +
# prompt-scanner + its scanner backend plugins, all one process). No Python,
# no separate sidecar/interceptor process — forward-proxy's
# openai-interceptor-plugin resolves `PromptScannerClientV1` in-process via
# `ClientHub`.
#
# Note: prompt-injection detection is out of scope for now — the old
# GLiNER2-sidecar-backed prompt-injection-plugin (Python) was removed
# entirely. See gears/onnx-runner/docs/DESIGN.md's decision #1.
#
# Normally invoked via `make genai-security` (see root Makefile).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# The workspace pins `ort` with `load-dynamic`, so ONNX Runtime is resolved
# from ORT_DYLIB_PATH at process start. Unset or wrong, `ort` 2.0.0-rc.12 hangs
# instead of erroring — onnx-runner bounds that with a timeout, but a startup
# that fails after a minute is still a bad first experience. Fetch on demand
# and leave any operator setting alone.
if [[ -z "${ORT_DYLIB_PATH:-}" ]]; then
  if ORT_DYLIB_PATH="$(bash "$REPO_ROOT/scripts/fetch-onnxruntime.sh" --path)"; then
    export ORT_DYLIB_PATH
  else
    echo "warning: could not provision an ONNX Runtime; onnx-runner will fail to start." >&2
    echo "         Run: make onnxruntime-fetch" >&2
  fi
fi

CONFIG_PATH="${CONFIG_PATH:-config/genai-security.yaml}"
FEATURES="${FEATURES:-forward-proxy-example,demo-policy-console,static-tenants,static-authz}"

API_HOST="${API_HOST:-127.0.0.1}"
API_PORT="${API_PORT:-8087}"
API_PREFIX="${API_PREFIX:-/cf}"
API_BASE="http://${API_HOST}:${API_PORT}${API_PREFIX}"
API_READY_TRIES="${API_READY_TRIES:-300}"

PROXY_HOST="${PROXY_HOST:-127.0.0.1}"
PROXY_PORT="${PROXY_PORT:-8080}"

# Raw stdout/stderr of the server (cargo build output, panics) — separate
# from the gear's own structured log
# (~/.cf-gears-genai-security/logs/cf-gears.log, per
# config/genai-security.yaml's `logging` section), which only starts once
# the server has finished booting. Check both when troubleshooting a failed
# startup.
LOG_DIR="$REPO_ROOT/logs/genai-security"
mkdir -p "$LOG_DIR"
SERVER_LOG="$LOG_DIR/cf-gears-example-server.log"
: > "$SERVER_LOG"

BACKGROUND_PIDS=()

cleanup() {
    echo ""
    echo "==> shutting down background processes..."
    for pid in "${BACKGROUND_PIDS[@]:-}"; do
        [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
    done
    wait 2>/dev/null || true
}
trap cleanup EXIT INT TERM

wait_for_http() {
    local url="$1" name="$2" tries="$3"
    for _ in $(seq 1 "$tries"); do
        if curl -sf -o /dev/null "$url"; then
            echo "==> $name is up ($url)"
            return 0
        fi
        sleep 1
    done
    echo "==> ERROR: $name did not answer $url within ${tries}s" >&2
    return 1
}

mkdir -p "$REPO_ROOT/data/genai-security"

echo "==> [1/2] starting cf-gears-example-server (features: $FEATURES)"
echo "    api listening on:   $API_BASE"
echo "    proxy listening on: http://$PROXY_HOST:$PROXY_PORT"
echo "    log (this script):  $SERVER_LOG"
echo "    log (the gear):     ~/.cf-gears-genai-security/logs/cf-gears.log (once booted)"
echo "    (point browser/OS proxy settings at $PROXY_HOST:$PROXY_PORT and trust its CA —"
echo "    see docs/genai-security/SETUP.md section 4 for the browser steps;"
echo "    forward-proxy persists its own CA under ./data/genai-security/)"
echo "    (first run compiles from scratch — can take a few minutes)"
# FORCE_COLOR=1: piping through `tee` below makes stdout/stderr a pipe, not
# a TTY, so toolkit's own TTY-autodetection (`supports_color`) would
# otherwise disable ANSI colors. FORCE_COLOR overrides that. (The saved log
# file under $SERVER_LOG ends up with raw ANSI escapes too — `less -R` or
# `cat -v` displays it fine; strip with `sed 's/\x1b\[[0-9;]*m//g'` if you
# need a plain-text copy.)
(cd "$REPO_ROOT" && exec env FORCE_COLOR=1 cargo run -p cf-gears-example-server --features "$FEATURES" -- --config "$CONFIG_PATH" run) \
    > >(tee -a "$SERVER_LOG") 2>&1 &
SERVER_PID="$!"
BACKGROUND_PIDS+=("$SERVER_PID")
wait_for_http "$API_BASE/healthz" "cf-gears-example-server" "$API_READY_TRIES"

echo ""
if [[ ",$FEATURES," == *,demo-policy-console,* ]]; then
    # demo-policy-console is prompt-scanner's resolved PolicyProvider in this
    # build (see gears/prompt-scanner/docs/POLICY_PROVIDER_AND_DEMO_CONSOLE.md)
    # — it seeds enabled_backends from its own config on boot.
    echo "==> [2/2] demo-policy-console is enabled — enabled_backends is config-seeded"
    echo "    open the control panel: $API_BASE/demo-policy-console/"
else
    # No policy provider plugin: prompt-scanner serves its config's
    # fallback_policy to every tenant (config/gears.d/prompt-scanner.yaml).
    echo "==> [2/2] scanner backends come from prompt-scanner's fallback_policy"
    echo "    enabled_backends=[cf.builtin.always_allow, cf.builtin.regex_scanner, cf.builtin.onnx_pii]"
fi
echo "    (cf.builtin.onnx_pii is registered but won't produce findings until a real"
echo "    model is configured in onnx-runner — see gears/onnx-runner/docs/DESIGN.md)"

echo ""
echo "==> up and running. Ctrl+C tears down the server."
echo "    logs: $LOG_DIR/ (this script's stdout/stderr capture)"
echo "          ~/.cf-gears-genai-security/logs/cf-gears.log (the gear's own structured log)"
wait "$SERVER_PID"
