#!/usr/bin/env bash
# Enforce the per-layer error-handling policy documented in coco-rs/CLAUDE.md.
#
# Three tiers (classified by crate path under coco-rs/):
#
#   Tier 1 — terminals     app/cli, app/tui, exec/*, tests/*
#                          anyhow OK; no rules enforced.
#
#   Tier 2 — libraries     utils/*, vercel-ai/*, bridge, retrieval, tui-ui
#                          - no `coco-error` / `snafu` in [dependencies]
#                          - public API must not return `anyhow::Result`
#
#   Tier 3 — main trunk    everything else under coco-rs/
#                          - must declare `coco-error` in [dependencies]
#                          - no `anyhow` in [dependencies]
#
# Exemptions:
#   - common/error              IS coco-error (no self-dep)
#   - common/stack-trace-macro  proc-macro crate (no runtime deps)
#   - common/types              foundation types, zero internal deps, no errors
#   - common/llm-types          pure vercel-ai re-export shim, no errors
#   - common/model-card         vendor model facts, zero internal deps, no errors
#   - services/mcp-types        auto-generated wire types, no errors
#   - keybindings               leaf parser, ParseError is intentionally
#                               stringly per parser.rs (user-facing messages,
#                               not matchable variants — no StatusCode story)
#   - app/state                 state tree, no error-returning surface
#   - core/messages             pure data normalization, no error-returning surface
#   - [dev-dependencies]        anyhow stays useful in tests
#   - *.test.rs / tests/ / main.rs / pub(crate) etc.
#
# Existing violations are grandfathered in scripts/error-policy-allowlist.txt
# (one "<crate-relpath-or-fileloc> <rule-id>" per line). New violations not
# in the allowlist fail. Allowlist entries that are no longer violations
# also fail — forces the list to shrink as crates are migrated.
#
# Wired into `just quick-check` and `just pre-commit`. Standalone:
#   ./scripts/check-error-policy.sh

set -euo pipefail

cd "$(dirname "$0")/.."

ALLOWLIST_FILE="scripts/error-policy-allowlist.txt"

# -----------------------------------------------------------------------------
# Tier classification
# -----------------------------------------------------------------------------

is_tier1_terminal() {
    case "$1" in
        app/cli|app/tui|exec/*|tests/*) return 0 ;;
        *) return 1 ;;
    esac
}

is_tier2_lib() {
    case "$1" in
        utils/*|vercel-ai/*|bridge|retrieval|tui-ui|tui-markdown|tui-mermaid) return 0 ;;
        *) return 1 ;;
    esac
}

is_tier3_main_trunk() {
    is_tier1_terminal "$1" && return 1
    is_tier2_lib "$1" && return 1
    case "$1" in
        # Self-dep / proc-macro / pure-types crates have no runtime errors
        # to classify, so the "must depend on coco-error" rule does not apply.
        common/error|common/stack-trace-macro) return 1 ;;
        common/types|services/mcp-types)      return 1 ;;
        common/llm-types)                     return 1 ;;
        common/model-card)                    return 1 ;;
        # Infallible diagnostics sink: no Result-returning public API
        # (capture/finish/sink swallow their own I/O errors after a
        # tracing::warn!), so the "must depend on coco-error" rule does
        # not apply — same rationale as services/mcp-types.
        services/wire-dump)                   return 1 ;;
        keybindings)                          return 1 ;;
        app/state|core/messages)              return 1 ;;
        *) return 0 ;;
    esac
}

# -----------------------------------------------------------------------------
# Cargo.toml parsing
# -----------------------------------------------------------------------------

# True iff the plain [dependencies] section of $1 declares an unconditional
# dep on $2. Skips:
#   - [dev-dependencies], [build-dependencies], target-conditional sections
#   - deps marked `optional = true` (single-line table form) — these are
#     feature-gated and only land in the default build when a feature opts in.
deps_contains() {
    local toml="$1" crate="$2"
    awk -v crate="$crate" '
        /^\[dependencies\][[:space:]]*$/  { in_deps = 1; next }
        /^\[/                              { in_deps = 0; next }
        in_deps {
            # match `crate = ...` or `crate.workspace = true`
            if ($0 ~ "^" crate "([[:space:]]|=|\\.)") {
                if ($0 ~ /optional[[:space:]]*=[[:space:]]*true/) next
                found = 1; exit
            }
        }
        END { exit !found }
    ' "$toml"
}

# -----------------------------------------------------------------------------
# Rule checks — each prints one violation per line on stdout.
# -----------------------------------------------------------------------------

check_dep_rules() {
    find . -name Cargo.toml -not -path './target/*' -not -path './Cargo.toml' 2>/dev/null \
    | while IFS= read -r toml; do
        local rel="${toml#./}"
        local crate_dir="${rel%/Cargo.toml}"

        if is_tier2_lib "$crate_dir"; then
            if deps_contains "$toml" "coco-error"; then
                echo "$crate_dir tier2-deps-cocoerror"
            fi
            if deps_contains "$toml" "snafu"; then
                echo "$crate_dir tier2-deps-snafu"
            fi
            continue
        fi

        if is_tier3_main_trunk "$crate_dir"; then
            if ! deps_contains "$toml" "coco-error"; then
                echo "$crate_dir tier3-needs-cocoerror"
            fi
            if deps_contains "$toml" "anyhow"; then
                echo "$crate_dir tier3-deps-anyhow"
            fi
        fi
    done
}

check_pubfn_rule() {
    find utils vercel-ai bridge retrieval tui-ui tui-markdown tui-mermaid -name '*.rs' -not -path '*/target/*' 2>/dev/null \
    | grep -Ev '\.test\.rs$|/tests\.rs$|/tests/|/main\.rs$' \
    | while IFS= read -r path; do
        awk -v file="$path" '
            /pub( |\()(unsafe +)?(async +)?fn /        { in_sig = 1 }
            in_sig && /anyhow::Result/                 { print file ":" NR " tier2-pubfn-anyhow"; in_sig = 0; next }
            /\{/                                       { in_sig = 0 }
        ' "$path"
    done
}

# -----------------------------------------------------------------------------
# Diff against allowlist
# -----------------------------------------------------------------------------

violations=$(
    {
        check_dep_rules
        check_pubfn_rule
    } | LC_ALL=C sort -u
)

allowlist=""
if [ -f "$ALLOWLIST_FILE" ]; then
    allowlist=$(grep -Ev '^[[:space:]]*(#|$)' "$ALLOWLIST_FILE" | LC_ALL=C sort -u || true)
fi

new_violations=$(LC_ALL=C comm -23 <(echo "$violations") <(echo "$allowlist") | grep -v '^$' || true)
stale_entries=$(LC_ALL=C comm -13 <(echo "$violations") <(echo "$allowlist") | grep -v '^$' || true)

exit_code=0

if [ -n "$new_violations" ]; then
    echo "✗ New error-policy violations (not in $ALLOWLIST_FILE):" >&2
    echo "$new_violations" | sed 's/^/    /' >&2
    echo "  → see coco-rs/CLAUDE.md → Error Handling for the three-tier policy." >&2
    echo "  → fix the violation; do NOT add new entries to the allowlist." >&2
    exit_code=1
fi

if [ -n "$stale_entries" ]; then
    echo "✗ Stale entries in $ALLOWLIST_FILE (no longer violated — please remove):" >&2
    echo "$stale_entries" | sed 's/^/    /' >&2
    exit_code=1
fi

exit $exit_code
