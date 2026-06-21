#!/usr/bin/env bash
# Enforce the coco-tui-ui presentational seam.
#
# Rule: `coco-tui-ui` is a pure, domain-free, i18n-free presentational
# crate ("view-models in, ratatui out"). Its dependency sections must stay
# within a small allowlist (ratatui + crossterm + unicode-* + leaf utils +
# platform clipboard). It MUST NOT depend on any coco domain/app crate
# (config, messages, types, state, query, context, keybindings, …) or on
# rust-i18n — any of those would let AppState, settings, or translation leak
# into the render layer.
#
# Scans every `[dependencies]`-family section — including
# `[target.'cfg(…)'.dependencies]`, `[build-dependencies]`, and the dotted
# `[dependencies.<name>]` form. Only `[dev-dependencies]` is exempt (tests and
# benches may pull domain crates; they don't ship in the production seam).
#
# Wired into `just check-seam` (and thus quick-check / pre-commit). Run alone:
#   ./scripts/check-tui-ui-seam.sh
#
# Non-zero exit + offending dep names on violation; silent + status 0 when clean.

set -euo pipefail

cd "$(dirname "$0")/.."

manifest="tui-ui/Cargo.toml"
allow_re='^(ratatui|crossterm|unicode-width|unicode-segmentation|tracing|base64|tokio|arboard|libc|coco-utils-string|coco-utils-common)$'

deps=$(awk '
    # Section headers decide whether we are inside a checked dependency
    # block; the dotted single-dep form ([dependencies.<name>]) is handled
    # inline. dev-dependencies (plain or target-gated) are exempt.
    /^\[/ {
        inblock = 0
        if ($0 ~ /dev-dependencies/) next
        if ($0 ~ /^\[(target\.[^]]*\.)?(build-)?dependencies\]/) { inblock = 1; next }
        if ($0 ~ /^\[(target\.[^]]*\.)?(build-)?dependencies\.[A-Za-z0-9_-]+\]/) {
            name = $0
            sub(/\].*$/, "", name)
            sub(/^.*dependencies\./, "", name)
            print name
        }
        next
    }
    inblock && /^[A-Za-z0-9_-]+/ {
        name = $0
        sub(/[[:space:]]*[.=].*$/, "", name)
        print name
    }
' "$manifest")

violations=$(printf '%s\n' "$deps" | grep -Ev "$allow_re" | grep -v '^$' || true)

if [ -n "$violations" ]; then
    # Derive the human-readable allowlist from allow_re so the two can't drift.
    allow_list=$(printf '%s' "$allow_re" | sed -e 's/^\^(//' -e 's/)\$$//' -e 's/|/, /g')
    echo "✗ coco-tui-ui depends on crates outside the presentational allowlist:" >&2
    echo "$violations" | sed 's/^/    /' >&2
    echo "  → the render crate must stay domain-free and i18n-free." >&2
    echo "  → allowed: $allow_list" >&2
    echo "  → presentation that needs AppState/messages/config/i18n belongs in app/tui." >&2
    exit 1
fi
