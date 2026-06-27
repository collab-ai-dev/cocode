#!/usr/bin/env bash
# Drop-in `coco` replacement that spawns the `sdk_server_stdio` test
# binary instead of the full release build. Used by the Python coco-sdk
# e2e suite so a single missing `target/debug/coco` doesn't trigger
# an 8-minute cold compile.
#
# Two-step strategy:
#
#   1. `cargo test --no-run -p coco-tests-live --test sdk_server_stdio`
#      compiles the test target (idempotent, ~1 min cold, milliseconds
#      warm) and emits the binary path via `--message-format=json`.
#   2. `exec` straight into that binary with the translated args.
#
# Why exec the binary instead of `cargo test ...` for the run step:
# cargo wraps the test process in a way that doesn't propagate
# Python's `asyncio.subprocess` stdin pipe — the child sees immediate
# EOF and shuts down before the SDK client can drive a turn. Exec'ing
# the binary directly inherits the pipe cleanly.
#
# Translates the binary's CLI shape:
#
#     coco --models.main <provider>/<model_id> sdk [other flags]
#
# into:
#
#     COCO_SDK_STDIO_RUN=1 <test_binary> \
#         --provider <provider> --model <model_id>
#
# Caveats
# -------
# * Only the `--models.main provider/model_id` flag and the `sdk` subcommand
#   are honored — other binary flags are silently dropped. The Python
#   transport doesn't pass anything else today, but if you add `--cwd`
#   or `--max-turns` etc. you'll need to extend this.

set -euo pipefail

MODEL=""
SAW_SDK=false
while [[ $# -gt 0 ]]; do
    case "$1" in
        --models.main)
            MODEL="${2:-}"
            shift 2 || { echo "[coco-sdk-via-cargo] --models.main requires a value" >&2; exit 64; }
            ;;
        --models.main=*)
            MODEL="${1#--models.main=}"
            shift
            ;;
        sdk)
            SAW_SDK=true
            shift
            ;;
        --)
            shift
            ;;
        *)
            # Best-effort: drop unknown flags rather than failing. The
            # Python transport sticks to the basics today; extend the
            # matcher above when that changes.
            shift
            ;;
    esac
done

if [[ "$SAW_SDK" != "true" ]]; then
    echo "[coco-sdk-via-cargo] expected the 'sdk' subcommand in argv" >&2
    exit 64
fi
if [[ -z "$MODEL" || "$MODEL" != */* ]]; then
    echo "[coco-sdk-via-cargo] expected --models.main provider/model_id, got: '$MODEL'" >&2
    exit 64
fi

PROVIDER="${MODEL%%/*}"
MODEL_ID="${MODEL#*/}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
COCO_RS_DIR="${SCRIPT_DIR}/.."

export COCO_SDK_STDIO_RUN=1

# Step 1: resolve the test binary path. Prefer a pre-built path from
# `COCO_SDK_STDIO_BIN` (set by `run-sdk-py-tests.sh` once per pytest
# invocation) so we don't pay cargo's per-test lock + json-decode cost
# (~200ms each, plus rare flakes when many wrapper invocations race
# back-to-back). Fall back to a JIT cargo build if the env var is
# unset (e.g. someone runs the wrapper standalone).
BIN_PATH="${COCO_SDK_STDIO_BIN:-}"
if [[ -z "$BIN_PATH" || ! -x "$BIN_PATH" ]]; then
    BIN_PATH=$(
        cargo test --no-run \
            --message-format=json \
            --manifest-path "${COCO_RS_DIR}/Cargo.toml" \
            -p coco-tests-live \
            --test sdk_server_stdio \
            2>/dev/null \
        | python3 -c '
import json, sys
for line in sys.stdin:
    try:
        msg = json.loads(line)
    except Exception:
        continue
    if (msg.get("reason") == "compiler-artifact"
            and msg.get("target", {}).get("name") == "sdk_server_stdio"):
        for f in msg.get("filenames", []) or []:
            print(f)
            sys.exit(0)
sys.exit(1)
'
    )
fi

if [[ -z "$BIN_PATH" || ! -x "$BIN_PATH" ]]; then
    echo "[coco-sdk-via-cargo] failed to locate sdk_server_stdio binary" >&2
    exit 70
fi

# Step 2: hand stdio to the binary directly. Cargo's process wrapper
# closes Python's asyncio stdin pipe prematurely; exec'ing the binary
# avoids that.
exec "$BIN_PATH" --provider "$PROVIDER" --model "$MODEL_ID"
