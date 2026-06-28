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
    bash "$DIR/generate_schemas.sh" --check
    bash "$DIR/generate_python.sh" --check
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
