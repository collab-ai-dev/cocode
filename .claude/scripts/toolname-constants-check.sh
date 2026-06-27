#!/usr/bin/env bash
# Tool name constants check — detects hardcoded MCP/worktree prefixes in changed Rust files.
# Only checks unique, zero-false-positive patterns. ToolName enum strings (Read, Bash, etc.)
# are NOT checked because they are common English words with too many legitimate uses.
# Exit 0 = pass, Exit 2 = fail (stderr fed to agent).

# Best-effort tracelog (see rust-quality-check.sh for rationale).
{ echo "$(date "+%FT%T%z") toolname-constants-check pid=$$" \
    >> /tmp/coco-stop-hook-trace.log; } 2>/dev/null || true

REPO_ROOT="${CLAUDE_PROJECT_DIR:?CLAUDE_PROJECT_DIR not set}"
cd "$REPO_ROOT"

# Get changed .rs files (excluding canonical definitions and test files)
CHANGED_RS=$(git diff --name-only HEAD 2>/dev/null | grep '\.rs$' \
  | grep -v 'common/protocol/src/tools/mod\.rs$' \
  | grep -v '\.test\.rs$' \
  | grep -v 'utils/git/src/worktree\.rs$' \
  || true)

if [ -z "$CHANGED_RS" ]; then
  exit 0
fi

VIOLATIONS=""

while IFS= read -r file; do
  [ -f "$file" ] || continue

  # Pattern 1: starts_with("mcp__") or strip_prefix("mcp__") without constant
  HITS=$(grep -nE '(starts_with|strip_prefix)\("mcp__"\)' "$file" 2>/dev/null \
    | grep -v 'MCP_TOOL_PREFIX' \
    || true)
  if [ -n "$HITS" ]; then
    VIOLATIONS+="  $file — use cocode_protocol::MCP_TOOL_PREFIX:\n$HITS\n\n"
  fi

  # Pattern 2: format!("mcp__...) MCP qualified name construction
  HITS=$(grep -n 'format!("mcp__' "$file" 2>/dev/null \
    | grep -v 'MCP_TOOL_PREFIX' \
    || true)
  if [ -n "$HITS" ]; then
    VIOLATIONS+="  $file — use MCP_TOOL_PREFIX + MCP_TOOL_SEPARATOR:\n$HITS\n\n"
  fi

  # Pattern 3: "agent/task-" worktree branch prefix hardcoded
  HITS=$(grep -n '"agent/task-"' "$file" 2>/dev/null \
    | grep -v 'AGENT_WORKTREE_BRANCH_PREFIX' \
    | grep -v 'const.*=.*"agent/task-"' \
    || true)
  if [ -n "$HITS" ]; then
    VIOLATIONS+="  $file — use AGENT_WORKTREE_BRANCH_PREFIX:\n$HITS\n\n"
  fi

done <<< "$CHANGED_RS"

if [ -n "$VIOLATIONS" ]; then
  cat >&2 <<AGENT_MSG
Hardcoded tool/prefix constants found in changed files.
Replace with protocol-defined constants:

  "mcp__"        → cocode_protocol::MCP_TOOL_PREFIX
  "__" (in MCP)  → cocode_protocol::MCP_TOOL_SEPARATOR
  "agent/task-"  → cocode_protocol::AGENT_WORKTREE_BRANCH_PREFIX

$(echo -e "$VIOLATIONS")
AGENT_MSG
  exit 2
fi

exit 0
