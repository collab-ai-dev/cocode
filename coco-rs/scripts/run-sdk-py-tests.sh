#!/usr/bin/env bash
# Run the coco-sdk Python suite (unit + e2e).
#
# Lazily bootstraps a venv at coco-sdk/python/.venv on first run so
# `just pre-commit` doesn't depend on each developer pre-installing
# pytest/pytest-asyncio/pydantic. The venv is reused on subsequent
# runs (cheap).
#
# Binary discovery for e2e tests (in priority order):
#   1. `COCO_PATH` if set and executable — attach to whatever build
#      the user pointed at; no rebuild. Use this to share a single
#      `target/debug/coco` across many test runs.
#   2. `coco-rs/target/debug/coco` if present — same idea, default
#      location.
#   3. Otherwise, if e2e tests would run (i.e. a `tests/e2e/` argument
#      or no test path was given) AND `DEEPSEEK_API_KEY` is set, point
#      `COCO_PATH` at `scripts/coco-sdk-via-cargo.sh`. That wrapper
#      spawns the `sdk_server_stdio` `harness = false` test binary via
#      `cargo test`, which compiles in ~1 min cold (vs. ~8 min for the
#      full `coco` binary) and re-runs in seconds when the workspace
#      is already warm.
#
# E2E tests self-skip when DEEPSEEK_API_KEY is unset — the suite is
# safe to wire into pre-commit unconditionally.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SDK_PYTHON_DIR="${SCRIPT_DIR}/../../coco-sdk/python"
COCO_RS_DIR="${SCRIPT_DIR}/.."
DEFAULT_BINARY="${COCO_RS_DIR}/target/debug/coco"

if [[ ! -d "${SDK_PYTHON_DIR}" ]]; then
    echo "[sdk-py-test] coco-sdk python dir not found at ${SDK_PYTHON_DIR}; skipping" >&2
    exit 0
fi

cd "${SDK_PYTHON_DIR}"

PY_BIN="${COCO_SDK_PY_BIN:-python3}"
if ! command -v "${PY_BIN}" >/dev/null 2>&1; then
    echo "[sdk-py-test] '${PY_BIN}' not on PATH; skipping. Set COCO_SDK_PY_BIN to override." >&2
    exit 0
fi

# Resolve / build coco binary for e2e tests.
needs_e2e=true
for arg in "$@"; do
    case "$arg" in
        # If the caller restricted to a non-e2e path we don't need the binary.
        tests/test_*|tests/test_*.py) needs_e2e=false ;;
    esac
done

if [[ -z "${COCO_PATH:-}" ]] && [[ -x "${DEFAULT_BINARY}" ]]; then
    export COCO_PATH="${DEFAULT_BINARY}"
fi

# When no full binary is available, prefer the `sdk_server_stdio` test
# target via `coco-sdk-via-cargo.sh` over a one-off `coco` build. Same
# functional surface (real SdkServer + QueryEngineRunner against live
# DeepSeek), an order of magnitude less compile time. See the script's
# header for the CLI translation it performs.
CARGO_WRAPPER="${SCRIPT_DIR}/coco-sdk-via-cargo.sh"
if [[ "${needs_e2e}" == "true" ]] && [[ -n "${DEEPSEEK_API_KEY:-}" ]] && [[ -z "${COCO_PATH:-}" ]]; then
    if [[ -x "${CARGO_WRAPPER}" ]]; then
        echo "[sdk-py-test] no coco binary found; using sdk_server_stdio test target via" >&2
        echo "[sdk-py-test]   ${CARGO_WRAPPER}" >&2
        export COCO_PATH="${CARGO_WRAPPER}"
    else
        echo "[sdk-py-test] no coco binary and no cargo wrapper; falling back to" >&2
        echo "[sdk-py-test] building target/debug/coco for e2e (one-time, ~5-8 min cold)..." >&2
        (cd "${COCO_RS_DIR}" && cargo build -p coco-cli --bin coco --quiet)
        export COCO_PATH="${DEFAULT_BINARY}"
    fi
fi

# Pre-build + cache the sdk_server_stdio test binary path when we're
# routing through the cargo wrapper. Without this, every Python test
# invokes `cargo test --no-run` to discover the path — that's both
# wasteful (each invocation reacquires cargo's workspace lock) and
# subject to rare flakes where back-to-back invocations race. One
# build up-front, one env-var lookup per test.
if [[ "${COCO_PATH:-}" == "${CARGO_WRAPPER}" ]] && [[ -z "${COCO_SDK_STDIO_BIN:-}" ]]; then
    echo "[sdk-py-test] building sdk_server_stdio test binary (one-time, cache-then-reuse)..." >&2
    BIN_PATH=$(
        cd "${COCO_RS_DIR}" \
        && cargo test --no-run --message-format=json \
            -p coco-tests-live --test sdk_server_stdio 2>/dev/null \
        | python3 -c '
import json, sys
for line in sys.stdin:
    try: m = json.loads(line)
    except: continue
    if (m.get("reason") == "compiler-artifact"
            and m.get("target", {}).get("name") == "sdk_server_stdio"):
        for f in m.get("filenames", []) or []:
            print(f); sys.exit(0)
sys.exit(1)
'
    )
    if [[ -n "${BIN_PATH}" ]] && [[ -x "${BIN_PATH}" ]]; then
        export COCO_SDK_STDIO_BIN="${BIN_PATH}"
        echo "[sdk-py-test] cached sdk_server_stdio binary: ${COCO_SDK_STDIO_BIN}" >&2
    else
        echo "[sdk-py-test] WARN: failed to cache sdk_server_stdio binary; wrapper will rebuild per test" >&2
    fi
fi

if [[ -n "${COCO_PATH:-}" ]]; then
    echo "[sdk-py-test] using coco binary: ${COCO_PATH}" >&2
fi

VENV_DIR=".venv"
if [[ ! -d "${VENV_DIR}" ]]; then
    echo "[sdk-py-test] bootstrapping ${SDK_PYTHON_DIR}/${VENV_DIR}" >&2
    "${PY_BIN}" -m venv "${VENV_DIR}"
    "${VENV_DIR}/bin/python" -m pip install --quiet --upgrade pip
    "${VENV_DIR}/bin/python" -m pip install --quiet -e ".[dev]"
fi

# Refresh editable install + dev deps if pyproject.toml changed since
# the venv was created — keeps the recipe self-healing without an
# explicit `just sdk-py-install` step.
if [[ pyproject.toml -nt "${VENV_DIR}/pyvenv.cfg" ]]; then
    echo "[sdk-py-test] pyproject.toml changed since venv was built; reinstalling" >&2
    "${VENV_DIR}/bin/python" -m pip install --quiet -e ".[dev]"
    touch "${VENV_DIR}/pyvenv.cfg"
fi

exec "${VENV_DIR}/bin/python" -m pytest "$@"
