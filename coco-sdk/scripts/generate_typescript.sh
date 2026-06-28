#!/usr/bin/env bash
# Generate TypeScript protocol types from JSON Schema.
#
# Usage:
#   ./coco-sdk/scripts/generate_typescript.sh           # regenerate in place
#   ./coco-sdk/scripts/generate_typescript.sh --check   # exit 1 on drift

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SCHEMA_DIR="$REPO_ROOT/coco-sdk/schemas/json"
SCRIPTS_DIR="$(cd "$(dirname "$0")" && pwd)"
PROTOCOL_PATH="$REPO_ROOT/coco-sdk/typescript/src/generated/protocol.ts"

CHECK_MODE=false
for arg in "$@"; do
    case "$arg" in
        --check) CHECK_MODE=true ;;
        -h|--help)
            sed -n '2,8p' "$0"
            exit 0
            ;;
        *)
            echo "error: unknown flag '$arg' (use --help)" >&2
            exit 2
            ;;
    esac
done

if [ ! -f "$SCHEMA_DIR/server_notification.json" ]; then
    echo "Schema files missing in $SCHEMA_DIR." >&2
    echo "Run: ./coco-sdk/scripts/generate_schemas.sh --force" >&2
    exit 1
fi

run_pipeline() {
    local out_protocol="$1"
    python3 "$SCRIPTS_DIR/postprocess_typescript.py" "$SCHEMA_DIR" "$out_protocol"
    if command -v prettier &>/dev/null; then
        prettier --write "$out_protocol" >/dev/null 2>&1 || true
    fi
}

if $CHECK_MODE; then
    TMP_OUT="$(mktemp -d)"
    trap 'rm -rf "$TMP_OUT"' EXIT
    STAGING_PROTOCOL="$TMP_OUT/protocol.ts"
    echo "==> Running TypeScript codegen into $TMP_OUT (check mode)..."
    run_pipeline "$STAGING_PROTOCOL" >/dev/null
    if ! diff -q "$PROTOCOL_PATH" "$STAGING_PROTOCOL" >/dev/null 2>&1; then
        echo "ERROR: protocol.ts is out of date." >&2
        echo "       Run: ./coco-sdk/scripts/generate_typescript.sh" >&2
        diff -u "$PROTOCOL_PATH" "$STAGING_PROTOCOL" | head -80
        exit 1
    fi
    echo "==> OK: protocol.ts is up-to-date."
    exit 0
fi

echo "==> Generating TypeScript types from $SCHEMA_DIR..."
run_pipeline "$PROTOCOL_PATH"
echo "==> Done. Generated types in: $PROTOCOL_PATH"
