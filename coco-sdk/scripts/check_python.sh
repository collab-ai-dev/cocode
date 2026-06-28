#!/usr/bin/env bash
# Check the coco Python SDK formatting, unit tests, and generated artifacts.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PY_ROOT="$REPO_ROOT/coco-sdk/python"

cd "$PY_ROOT"

ruff format --check .
ruff check .
PYTHONPATH=src python3 -m pytest tests/test_*.py

cd "$REPO_ROOT"

./coco-sdk/scripts/generate_schemas.sh --check --quiet
./coco-sdk/scripts/generate_python.sh --check
