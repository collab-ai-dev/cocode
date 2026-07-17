#!/usr/bin/env bash
# Enforce that coco-agent-host stays protocol-neutral: no dependency on the TUI
# surface (coco-tui) or the SDK transport crate (coco-sdk-server). TUI/SDK/
# headless composition lives in coco-cli (app/cli/src/{tui,headless,sdk}) and the
# SDK transport in coco-sdk-server; agent-host exposes protocol-neutral host
# operations only. See Phase G of
# docs/internal/multi-session-app-server/remediation-plan.md.
#
# Scans every `[dependencies]`-family section (including target-gated and the
# dotted `[dependencies.<name>]` form). `[dev-dependencies]` is exempt.
#
# Wired into `just check-seam` (and thus quick-check / pre-commit). Run alone:
#   ./scripts/check-agent-host-seam.sh
#
# Non-zero exit + offending dep on violation; silent + status 0 when clean.

set -euo pipefail

cd "$(dirname "$0")/.."

forbidden_re='^(coco-tui|coco-sdk-server)$'
manifest=app/agent-host/Cargo.toml

bad=$(awk -v forbidden="$forbidden_re" '
    /^\[/ {
        inblock = 0
        if ($0 ~ /dev-dependencies/) next
        if ($0 ~ /^\[(target\.[^]]*\.)?(build-)?dependencies\]/) { inblock = 1; next }
        if ($0 ~ /^\[(target\.[^]]*\.)?(build-)?dependencies\.[A-Za-z0-9_-]+\]/) {
            name = $0
            sub(/\].*$/, "", name)
            sub(/^.*dependencies\./, "", name)
            if (name ~ forbidden) print name
        }
        next
    }
    inblock && /^[A-Za-z0-9_-]+/ {
        name = $1
        sub(/[^A-Za-z0-9_-].*$/, "", name)
        if (name ~ forbidden) print name
    }
' "$manifest")

if [ -n "$bad" ]; then
    echo "agent-host seam violation in $manifest:"
    echo "$bad" | sed 's/^/  forbidden dependency: /'
    echo
    echo "coco-agent-host must stay protocol-neutral. TUI/SDK composition lives"
    echo "in coco-cli and coco-sdk-server; move surface-specific adapters there."
    exit 1
fi
