#!/usr/bin/env bash
# Full regeneration pipeline: Rust → JSON Schema → Python types.
#
# Usage:
#   ./coco-sdk/scripts/generate_all.sh          # Generate all
#   ./coco-sdk/scripts/generate_all.sh --check   # Verify generated files are up-to-date

set -euo pipefail

DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$DIR/../.." && pwd)"
CHECK_MODE=false

if [[ "${1:-}" == "--check" ]]; then
    CHECK_MODE=true
fi

if $CHECK_MODE; then
    echo "=== Check mode: verifying generated files are up-to-date ==="

    # Save current generated file
    PYTHON_OUTPUT="$REPO_ROOT/coco-sdk/python/src/coco_sdk/generated/protocol.py"
    TEMP_DIR="$(mktemp -d)"
    trap 'rm -rf "$TEMP_DIR"' EXIT

    cp "$PYTHON_OUTPUT" "$TEMP_DIR/protocol.py.before"

    # Regenerate
    bash "$DIR/generate_schemas.sh" > /dev/null 2>&1
    bash "$DIR/generate_python.sh" > /dev/null 2>&1

    # Compare
    if ! diff -q "$TEMP_DIR/protocol.py.before" "$PYTHON_OUTPUT" > /dev/null 2>&1; then
        echo "ERROR: Generated protocol.py is out of date!"
        echo "Run: ./coco-sdk/scripts/generate_all.sh"
        diff -u "$TEMP_DIR/protocol.py.before" "$PYTHON_OUTPUT" || true
        # Restore original
        cp "$TEMP_DIR/protocol.py.before" "$PYTHON_OUTPUT"
        exit 1
    fi

    echo "=== All generated files are up-to-date ==="
    exit 0
fi

echo "=== Step 1: Generate JSON Schema from Rust ==="
bash "$DIR/generate_schemas.sh"

echo ""
echo "=== Step 2: Generate Python types ==="
bash "$DIR/generate_python.sh"

echo ""
echo "=== Done ==="
