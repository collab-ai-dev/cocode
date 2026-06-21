#!/usr/bin/env bash
# Cargo passes the real rustc path as argv[1]. Use sccache when it is installed;
# otherwise run rustc directly so local builds work without optional tooling.

set -euo pipefail

if command -v sccache >/dev/null 2>&1; then
    exec sccache "$@"
fi

exec "$@"
