#!/usr/bin/env bash
# Check the coco Python SDK formatting, unit tests, and generated artifacts.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PY_ROOT="$REPO_ROOT/coco-sdk/python"
SDK_VENV_DIR="$PY_ROOT/.venv"
RUFF_VERSION="0.15.20"
RUFF_REQUIREMENT="ruff==$RUFF_VERSION"
PY_BIN="${COCO_SDK_PY_BIN:-python3}"

bootstrap_sdk_venv() {
    if ! command -v "$PY_BIN" >/dev/null 2>&1; then
        echo "error: '$PY_BIN' not on PATH. Set COCO_SDK_PY_BIN to override." >&2
        return 1
    fi

    if [[ ! -d "$SDK_VENV_DIR" ]]; then
        echo "==> bootstrapping $SDK_VENV_DIR" >&2
        "$PY_BIN" -m venv "$SDK_VENV_DIR"
        "$SDK_VENV_DIR/bin/python" -m pip install --quiet --upgrade pip
        "$SDK_VENV_DIR/bin/python" -m pip install --quiet -e "$PY_ROOT[dev]"
    fi

    if [[ "$PY_ROOT/pyproject.toml" -nt "$SDK_VENV_DIR/pyvenv.cfg" ]]; then
        echo "==> pyproject.toml changed since venv was built; reinstalling" >&2
        "$SDK_VENV_DIR/bin/python" -m pip install --quiet -e "$PY_ROOT[dev]"
        touch "$SDK_VENV_DIR/pyvenv.cfg"
    fi
}

ruff_version_matches() {
    local candidate="$1"
    [[ "$("$candidate" --version 2>/dev/null)" == "ruff $RUFF_VERSION" ]]
}

install_ruff() {
    local venv_ruff="$SDK_VENV_DIR/bin/ruff"
    echo "==> installing $RUFF_REQUIREMENT into $SDK_VENV_DIR..." >&2
    if [[ ! -x "$SDK_VENV_DIR/bin/python" ]]; then
        python3 -m venv "$SDK_VENV_DIR"
    fi

    "$SDK_VENV_DIR/bin/python" -m pip install --upgrade "$RUFF_REQUIREMENT" >&2
    if [[ -x "$venv_ruff" ]] && ruff_version_matches "$venv_ruff"; then
        printf '%s\n' "$venv_ruff"
        return 0
    fi

    echo "error: installed $RUFF_REQUIREMENT but $venv_ruff is unavailable or has the wrong version" >&2
    return 1
}

resolve_ruff() {
    local venv_ruff="$SDK_VENV_DIR/bin/ruff"
    if [[ -x "$venv_ruff" ]]; then
        if ruff_version_matches "$venv_ruff"; then
            printf '%s\n' "$venv_ruff"
            return 0
        fi
        echo "==> $venv_ruff is not ruff $RUFF_VERSION; updating SDK venv..." >&2
        install_ruff
        return
    fi

    if command -v ruff >/dev/null 2>&1; then
        local path_ruff
        path_ruff="$(command -v ruff)"
        if ruff_version_matches "$path_ruff"; then
            printf '%s\n' "$path_ruff"
            return 0
        fi
        echo "==> $path_ruff is not ruff $RUFF_VERSION; using SDK venv instead..." >&2
        install_ruff
        return
    fi

    echo "==> ruff not found; using SDK venv..." >&2
    install_ruff
}

bootstrap_sdk_venv
RUFF_BIN="$(resolve_ruff)"

cd "$PY_ROOT"

"$RUFF_BIN" format --check .
"$RUFF_BIN" check .
PYTHONPATH=src "$SDK_VENV_DIR/bin/python" -m pytest tests/test_*.py

cd "$REPO_ROOT"

./coco-sdk/scripts/generate_schemas.sh --check --quiet
./coco-sdk/scripts/generate_python.sh --check
