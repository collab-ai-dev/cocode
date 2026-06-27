#!/usr/bin/env bash
# Generate Python Pydantic models from JSON Schema.
#
# Reads `coco-sdk/schemas/json/*` (run `generate_schemas.sh` first to
# refresh from Rust) and emits:
#   * `coco-sdk/python/src/coco_sdk/generated/protocol.py` — Pydantic
#     models with tagged-union accessors, type aliases, str enums.
#   * `coco-sdk/python/src/coco_sdk/__init__.py` — re-exports
#     hand-written + generated names; deduped, shadow-aware.
#
# Usage:
#   ./coco-sdk/scripts/generate_python.sh           # regenerate in place
#   ./coco-sdk/scripts/generate_python.sh --check   # exit 1 on any drift (CI mode)
#
# `--check` runs the generator into a tempdir and diffs every output
# against the committed copy. CI should run this so a drifted protocol
# layer fails the PR rather than failing in production.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SCHEMA_DIR="$REPO_ROOT/coco-sdk/schemas/json"
SCRIPTS_DIR="$(cd "$(dirname "$0")" && pwd)"
PROTOCOL_PATH="$REPO_ROOT/coco-sdk/python/src/coco_sdk/generated/protocol.py"
INIT_PATH="$REPO_ROOT/coco-sdk/python/src/coco_sdk/__init__.py"

CHECK_MODE=false
for arg in "$@"; do
    case "$arg" in
        --check) CHECK_MODE=true ;;
        -h|--help)
            sed -n '2,17p' "$0"
            exit 0
            ;;
        *)
            echo "error: unknown flag '$arg' (use --help)" >&2
            exit 2
            ;;
    esac
done

if [ ! -d "$SCHEMA_DIR" ]; then
    echo "Schema directory not found at $SCHEMA_DIR." >&2
    echo "Run: ./coco-sdk/scripts/generate_schemas.sh" >&2
    exit 1
fi

if [ ! -f "$SCHEMA_DIR/server_notification.json" ]; then
    echo "Schema files missing in $SCHEMA_DIR." >&2
    echo "Run: ./coco-sdk/scripts/generate_schemas.sh --force" >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# Run the three-phase pipeline. Same body for regen and --check, only the
# output paths differ. ruff (if present) auto-formats; without it the diff
# may still pass because the generators emit consistent style.
# ---------------------------------------------------------------------------
run_pipeline() {
    local out_protocol="$1"
    local out_init_dir="$2"
    mkdir -p "$(dirname "$out_protocol")" "$out_init_dir/coco_sdk"
    # The append/regen scripts read from the protocol path's parent
    # parent directory (`<root>/python`) to scan src/ + tests/, so they
    # always work against the canonical source tree. In --check mode we
    # only redirect the *outputs*; reads still come from the live tree.
    python3 "$SCRIPTS_DIR/postprocess_python.py" "$SCHEMA_DIR" "$out_protocol"
    python3 "$SCRIPTS_DIR/append_stubs.py" "$REPO_ROOT/coco-sdk/python" "$out_protocol"
    if command -v ruff &>/dev/null; then
        ruff format "$out_protocol" >/dev/null 2>&1 || true
    fi
    # regen_init.py writes `__init__.py` next to the protocol's parent
    # directory (i.e. `<protocol_dir>/../__init__.py`). For --check we
    # want it in our staging dir instead — copy the generated protocol
    # into the staging tree's expected location first.
    if [[ "$out_init_dir" != "$REPO_ROOT/coco-sdk/python/src" ]]; then
        local staging_proto="$out_init_dir/coco_sdk/generated/protocol.py"
        mkdir -p "$(dirname "$staging_proto")"
        cp "$out_protocol" "$staging_proto"
        python3 "$SCRIPTS_DIR/regen_init.py" "$staging_proto"
    else
        python3 "$SCRIPTS_DIR/regen_init.py" "$out_protocol"
    fi
}

if $CHECK_MODE; then
    TMP_OUT="$(mktemp -d)"
    trap 'rm -rf "$TMP_OUT"' EXIT
    echo "==> Running codegen into $TMP_OUT (check mode)..."

    STAGING_PROTOCOL="$TMP_OUT/protocol.py"
    STAGING_INIT="$TMP_OUT/staging/coco_sdk/__init__.py"
    run_pipeline "$STAGING_PROTOCOL" "$TMP_OUT/staging" >/dev/null

    fail=0
    for pair in \
        "protocol.py:$STAGING_PROTOCOL:$PROTOCOL_PATH" \
        "__init__.py:$TMP_OUT/staging/coco_sdk/__init__.py:$INIT_PATH"
    do
        IFS=':' read -r label fresh committed <<< "$pair"
        if ! diff -q "$committed" "$fresh" >/dev/null 2>&1; then
            echo "ERROR: $label is out of date." >&2
            echo "       Run: ./coco-sdk/scripts/generate_python.sh" >&2
            diff -u "$committed" "$fresh" | head -40
            fail=1
        fi
    done

    if [[ $fail -ne 0 ]]; then
        exit 1
    fi
    echo "==> OK: protocol.py and __init__.py are up-to-date."
    exit 0
fi

echo "==> Generating Python types from $SCHEMA_DIR..."
run_pipeline "$PROTOCOL_PATH" "$REPO_ROOT/coco-sdk/python/src"
echo "==> Done. Generated types in: $PROTOCOL_PATH"
