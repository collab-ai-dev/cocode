#!/usr/bin/env bash
# Workspace deps unification — child crates MUST declare every dep via
# `workspace = true`. No exceptions for external packages, internal path
# deps, or platform-specific blocks. The workspace root Cargo.toml owns
# every version; child crates only opt into features / optional flags.
#
# Rationale: if crate A pins "foo = 1.0" locally and crate B pins
# "foo = 2.0", Cargo.lock holds both versions and the binary fattens.
# The only way to make a single-version invariant structural is to
# centralize every dep at the workspace root.
#
# Exit 0 = pass, Exit 2 = fail (stderr fed to agent).

set -uo pipefail

# Best-effort tracelog (see rust-quality-check.sh for rationale).
{ echo "$(date "+%FT%T%z") workspace-deps-check pid=$$" \
    >> /tmp/coco-stop-hook-trace.log; } 2>/dev/null || true

REPO_ROOT="${CLAUDE_PROJECT_DIR:?CLAUDE_PROJECT_DIR not set}"
WORKSPACE_TOML="$REPO_ROOT/coco-rs/Cargo.toml"

if [ ! -f "$WORKSPACE_TOML" ]; then
  exit 0
fi

# Detect changed Cargo.toml files under coco-rs/ (tracked + untracked).
# Strip XY status flag and "old -> new" rename arrows.
CHANGED=$(git -C "$REPO_ROOT" status --porcelain 2>/dev/null \
  | sed 's/^...//' | sed 's/.* -> //' \
  | grep -E '^coco-rs/.+/Cargo\.toml$' || true)
if [ -z "$CHANGED" ]; then
  exit 0
fi

VIOLATIONS=""
while IFS= read -r rel_path; do
  abs_path="$REPO_ROOT/$rel_path"
  [ -f "$abs_path" ] || continue
  # Skip the workspace root itself (it defines [workspace.dependencies]).
  [ "$rel_path" = "coco-rs/Cargo.toml" ] && continue

  # Scan every dep section: [dependencies], [dev-dependencies],
  # [build-dependencies], and [target.'cfg(...)'.dependencies].
  # A line is a dep declaration if it starts with `<name> =` (regular
  # assignment). The dotted form `<name>.workspace = true` is excluded
  # automatically because it has a `.` before the `=`.
  HITS=$(awk '
    /^\[(dependencies|dev-dependencies|build-dependencies)\]/ { in_deps=1; next }
    /^\[target\..*\.(dependencies|dev-dependencies|build-dependencies)\]/ { in_deps=1; next }
    /^\[/ { in_deps=0; next }
    in_deps && /^[a-zA-Z_][a-zA-Z0-9_-]*[[:space:]]*=/ {
      if ($0 !~ /workspace[[:space:]]*=[[:space:]]*true/) {
        print NR ": " $0
      }
    }
  ' "$abs_path")

  if [ -n "$HITS" ]; then
    VIOLATIONS+="  $rel_path:\n"
    while IFS= read -r line; do
      VIOLATIONS+="    $line\n"
    done <<< "$HITS"
    VIOLATIONS+="\n"
  fi
done <<< "$CHANGED"

if [ -n "$VIOLATIONS" ]; then
  cat >&2 <<MSG
Workspace dep unification violation — every dep MUST inherit from
\`[workspace.dependencies]\`. Add the dep to coco-rs/Cargo.toml and
declare it in the child crate as:

  foo.workspace = true
  # or, if you need features / optional:
  foo = { workspace = true, features = [...], optional = true }

$(echo -e "$VIOLATIONS")
MSG
  exit 2
fi

exit 0
