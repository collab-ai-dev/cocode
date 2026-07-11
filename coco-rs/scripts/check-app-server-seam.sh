#!/usr/bin/env bash
# Enforce the AppServer layering seam (multi-session plan §5.3, D-11).
#
# Rule: engine and core crates MUST NOT depend on the server layer. Shared
# view types (envelopes, notification payloads, ids) live in `coco-types`,
# below both. A dependency the other way lets the wire view leak into the
# engine (the codex counter-lesson). This mirrors check-tui-ui-seam.sh.
#
# Checked crates: app/query, core/*, services/* (the engine + core layers).
# Forbidden deps: coco-app-server, coco-app-server-transport,
# coco-app-server-client, coco-app-runtime.
#
# Scans every `[dependencies]`-family section (including target-gated and the
# dotted `[dependencies.<name>]` form). `[dev-dependencies]` is exempt — tests
# may pull server crates.
#
# Wired into `just check-seam` (and thus quick-check / pre-commit). Run alone:
#   ./scripts/check-app-server-seam.sh
#
# Non-zero exit + offending crate/dep on violation; silent + status 0 when clean.

set -euo pipefail

cd "$(dirname "$0")/.."

forbidden_re='^(coco-app-server|coco-app-server-transport|coco-app-server-client|coco-app-runtime)$'

manifests=(app/query/Cargo.toml)
for dir in core services; do
    while IFS= read -r m; do
        manifests+=("$m")
    done < <(find "$dir" -name Cargo.toml -maxdepth 2 -type f)
done

violations=0
for manifest in "${manifests[@]}"; do
    [ -f "$manifest" ] || continue
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
        echo "AppServer seam violation in $manifest:"
        echo "$bad" | sed 's/^/  forbidden dependency: /'
        violations=1
    fi
done

if [ "$violations" -ne 0 ]; then
    echo
    echo "Engine/core crates must not depend on the server layer (plan §5.3)."
    echo "Move shared view types down to coco-types instead."
    exit 1
fi
