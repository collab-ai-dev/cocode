#!/usr/bin/env bash
# Enforce session cwd discipline on session-owned production code.
#
# Rule: runtime/session paths must use the cwd carried by session state or the
# CLI/headless startup boundaries. They must not re-read the process cwd,
# because a single app-server process can host sessions from different projects.
#
# This intentionally starts narrower than a workspace-wide clippy
# `disallowed-methods` rule: standalone tools and path utility crates still
# have legitimate process-cwd entrypoints. The guard below covers the
# session-owned crates that the multi-session app-server refactor has already
# moved to explicit cwd threading.

set -euo pipefail

cd "$(dirname "$0")/.."

roots=(
    app/cli/src
    app/query/src
    app/tui/src
    commands/src
    core/context/src
    core/permissions/src
    core/tool-runtime/src
    core/tools/src
    services/lsp/src
)

pattern='(std::env::current_dir|env::current_dir|AbsolutePathBuf::current_dir)[[:space:]]*\('

matches=$(
    while IFS= read -r path; do
        grep -nE "$pattern" "$path" 2>/dev/null | sed "s|^|$path:|" || true
    done < <(find "${roots[@]}" -name '*.rs' -not -name '*.test.rs' -print)
)

violations=$(
    printf '%s\n' "$matches" \
        | grep -Ev '^[^:]+:[0-9]+:[[:space:]]*(//|///|/\*)' \
        | grep -Ev '^app/cli/src/main\.rs:' \
        | grep -Ev '^app/cli/src/headless\.rs:[0-9]+:[[:space:]]*Some\(std::env::current_dir\(\)\?\)$' \
        | grep -v '^$' \
        || true
)

if [ -n "$violations" ]; then
    echo "✗ session-owned production code reads the process cwd:" >&2
    echo "$violations" | sed 's/^/    /' >&2
    echo "  → thread the session/startup cwd explicitly instead." >&2
    echo "  → allowed: app/cli/src/main.rs startup capture and headless run_chat convenience capture." >&2
    exit 1
fi
