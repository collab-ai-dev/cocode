#!/usr/bin/env bash
# No inline tests check — detects #[cfg(test)] mod tests { ... } in changed Rust files.
# Tests must always be in separate .test.rs files with #[path] attribute.
# Exit 0 = pass, Exit 2 = fail (stderr fed to agent).

# Best-effort tracelog (see rust-quality-check.sh for rationale).
{ echo "$(date "+%FT%T%z") no-inline-tests-check pid=$$" \
    >> /tmp/coco-stop-hook-trace.log; } 2>/dev/null || true

REPO_ROOT="${CLAUDE_PROJECT_DIR:?CLAUDE_PROJECT_DIR not set}"
cd "$REPO_ROOT"

# Get changed .rs files, excluding .test.rs and integration test directories
CHANGED_RS=$(git diff --name-only HEAD 2>/dev/null | grep '\.rs$' \
  | grep -v '\.test\.rs$' \
  | grep -v '/tests/' \
  || true)

if [ -z "$CHANGED_RS" ]; then
  exit 0
fi

VIOLATIONS=""

while IFS= read -r file; do
  [ -f "$file" ] || continue

  # Detect "mod tests {" (inline test module with opening brace, not "mod tests;")
  HITS=$(grep -n 'mod tests {' "$file" 2>/dev/null \
    | grep -v '^\s*//' \
    || true)
  if [ -n "$HITS" ]; then
    VIOLATIONS+="  $file:\n$HITS\n\n"
  fi

done <<< "$CHANGED_RS"

if [ -n "$VIOLATIONS" ]; then
  cat >&2 <<AGENT_MSG
Inline test modules found in changed files.
Extract tests to separate .test.rs files:

  #[cfg(test)]
  #[path = "<name>.test.rs"]
  mod tests;

$(echo -e "$VIOLATIONS")
AGENT_MSG
  exit 2
fi

exit 0
