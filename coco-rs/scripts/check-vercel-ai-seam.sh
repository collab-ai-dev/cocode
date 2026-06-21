#!/usr/bin/env bash
# Enforce the vercel-ai dual-seam.
#
# Rule: only the two designated seam crates may declare a `vercel-ai*`
# dependency. All other crates must reach AI SDK types through one of
# the seams — never `vercel_ai_provider::*` directly.
#
# Seam crates (steady state, both directly depend on vercel-ai by design):
#   - `common/llm-types/Cargo.toml`   — DTO seam: message + content
#                                       shapes consumed by domain crates
#                                       via `coco_llm_types::*`
#   - `services/inference/Cargo.toml` — runtime/client seam: LanguageModelV4
#                                       trait, Provider trait, ApiClient,
#                                       retry, auth, prompt-cache detection
#
# Switching SDK version edits both seams. Trying to collapse to a single
# seam would force runtime concerns into a types-only crate or schema
# concerns into a client crate — both worse than two narrow seams.
#
# `coco-messages` consumes content-part aliases via `coco_llm_types`.
# `coco-types` is intentionally vercel-ai-free at the source level.
#
# Wired into `just pre-commit` (justfile) and the Stop hook
# (.claude/scripts/rust-quality-check.sh). Safe to run standalone:
#   ./scripts/check-vercel-ai-seam.sh
#
# Exits with non-zero status (and prints offending Cargo.toml paths) when
# violations are found; silent + status 0 when clean.

set -euo pipefail

# Run from this script's parent directory so relative paths line up
# regardless of where the user invokes it.
cd "$(dirname "$0")/.."

# Allowed Cargo.toml owners of any `vercel-ai*` dep:
#   - the workspace root (`Cargo.toml`) where workspace deps are declared
#   - the `vercel-ai/` SDK crates themselves (they reference each other internally)
#   - `services/inference/Cargo.toml` — the single seam crate
allow_re='^(Cargo\.toml$|vercel-ai/|services/inference/Cargo\.toml$|common/llm-types/Cargo\.toml$)'

# A line in a Cargo.toml that declares a dep on any `vercel-ai*` crate.
# Matches both forms:
#   vercel-ai-anthropic = { workspace = true }
#   vercel-ai-anthropic.workspace = true
# Lines like `vercel-ai-stuff` in comments don't begin with the dep name,
# so the leading anchor is safe.
forbidden_dep_re='^[[:space:]]*vercel-ai[A-Za-z0-9_-]*([.][A-Za-z0-9_-]+)?[[:space:]]*='

violations=$(
    while IFS= read -r path; do
        if grep -qE "$forbidden_dep_re" "$path" 2>/dev/null; then
            printf '%s\n' "$path"
        fi
    done < <(find . -name Cargo.toml -not -path './target/*' -print)
)
violations=$(printf '%s' "$violations" | sed 's|^\./||' | grep -Ev "$allow_re" || true)

if [ -n "$violations" ]; then
    echo "✗ Cargo.toml depends on vercel-ai* outside the seam:" >&2
    echo "$violations" | sed 's/^/    /' >&2
    echo "  → only common/llm-types or services/inference may declare vercel-ai* deps." >&2
    echo "  → other crates must use coco_llm_types::* aliases (DTOs)." >&2
    echo "  → see common/llm-types/src/lib.rs for the canonical re-export list." >&2
    exit 1
fi
