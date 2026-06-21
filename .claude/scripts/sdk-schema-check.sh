#!/usr/bin/env bash
# SDK schema freshness gate — runs on Stop after editing protocol Rust files.
# Exit 0 = pass (schema is current), Exit 2 = fail (stderr fed to agent).
#
# Freshness check:
#   1. Detect if any protocol source files changed (git diff vs HEAD)
#   2. Build the schema exporter and export fresh JSON schemas to a temp dir
#   3. Compare temp schemas against the committed schemas (byte-for-byte)
#   4. If different → schema is stale → auto-regenerate schemas + Python SDK
#   5. If regeneration fails → exit 2 with actionable error for the agent

# Best-effort tracelog (see rust-quality-check.sh for rationale).
{ echo "$(date "+%FT%T%z") sdk-schema-check pid=$$" \
    >> /tmp/coco-stop-hook-trace.log; } 2>/dev/null || true

[ -f "/usr/local/cargo/env" ] && . "/usr/local/cargo/env"
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

REPO_ROOT="${CLAUDE_PROJECT_DIR:?CLAUDE_PROJECT_DIR not set}"
cd "$REPO_ROOT/cocode-rs"

# `CARGO_TARGET_DIR` deliberately unset — cargo's default workspace-relative
# `target/` already isolates worktrees.

# ── Step 1: detect protocol source changes ──
CHANGED_FILES=$(git diff --name-only HEAD 2>/dev/null || true)
PROTOCOL_CHANGED=false

for pattern in \
  'app-server-protocol/src/.*\.rs$' \
  'common/protocol/src/tools/' \
  'common/protocol/src/server_notification/' \
  'common/protocol/src/event_types\.rs$'; do
  if echo "$CHANGED_FILES" | grep -qE "$pattern"; then
    PROTOCOL_CHANGED=true
    break
  fi
done

if [ "$PROTOCOL_CHANGED" = false ]; then
  exit 0
fi

echo "Protocol types changed — checking SDK schema freshness..." >&2

# ── Step 2: export fresh schemas to temp and compare ──
TEMP_SCHEMA_DIR="$(mktemp -d)"
trap 'rm -rf "$TEMP_SCHEMA_DIR"' EXIT

# Build and run the schema exporter (writes to app-server-protocol/schema/json/)
EXPORT_OUTPUT=$(cargo run --bin export-app-server-schema 2>&1) || {
  cat >&2 <<'AGENT_MSG'
SDK schema export failed (cargo build error in export-app-server-schema).
The Rust protocol types have compilation errors that must be fixed first.

Action required:
  1. Check errors in cocode-rs/app-server-protocol/src/ files
  2. Run `just check` to see the compilation errors
  3. Fix the errors, then the schema will regenerate automatically on next stop
AGENT_MSG
  exit 2
}

# Copy fresh schemas to temp for comparison
cp "$REPO_ROOT/cocode-rs/app-server-protocol/schema/json/"*.json "$TEMP_SCHEMA_DIR/" 2>/dev/null

# Compare against committed SDK schemas
COMMITTED_SCHEMA_DIR="$REPO_ROOT/cocode-sdk/schemas/json"
if [ -d "$COMMITTED_SCHEMA_DIR" ] && diff -rq "$TEMP_SCHEMA_DIR" "$COMMITTED_SCHEMA_DIR" >/dev/null 2>&1; then
  echo "SDK schema is up-to-date." >&2
  exit 0
fi

# ── Step 3: schema is stale — regenerate ──
echo "SDK schema is stale. Regenerating Python SDK..." >&2

# Copy fresh schemas to SDK dir
mkdir -p "$COMMITTED_SCHEMA_DIR"
cp "$TEMP_SCHEMA_DIR/"*.json "$COMMITTED_SCHEMA_DIR/"

# Generate Python types from fresh schemas
GEN_OUTPUT=$(bash "$REPO_ROOT/cocode-sdk/scripts/generate_python.sh" 2>&1) || {
  cat >&2 <<AGENT_MSG
SDK Python type generation failed.

Schema JSON was exported successfully, but the Python codegen script failed:
$GEN_OUTPUT

Action required:
  1. Check cocode-sdk/scripts/postprocess_python.py for errors
  2. Run manually: ./cocode-sdk/scripts/generate_python.sh
  3. If the schema structure changed in a way the generator doesn't handle,
     update postprocess_python.py accordingly
AGENT_MSG
  exit 2
}

echo "SDK regenerated successfully. Updated files:" >&2
echo "  - cocode-rs/app-server-protocol/schema/json/*.json" >&2
echo "  - cocode-sdk/schemas/json/*.json" >&2
echo "  - cocode-sdk/python/src/cocode_sdk/generated/protocol.py" >&2
exit 0
