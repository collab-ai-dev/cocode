#!/usr/bin/env bash
# Rust quality gate — runs when Claude stops after editing Rust files.
# Exit 0 = pass (stop proceeds), Exit 2 = fail (stop blocked, stderr fed to agent).
#
# Modes:
#   (default) seam guard + fmt + scoped clippy. Heavy when Cargo.toml /
#             Cargo.lock changed — clippy.sh falls back to full-workspace
#             `--all-features --tests` (30-120s).
#   --light   seam guard + fmt only. Sub-second; intended for the Stop
#             hook so the gate stays responsive. Pair with manual
#             `just quick-check` before commit for the clippy pass.
#
# Targets `coco-rs/` because that is the active-development workspace
# (per coco-rs/CLAUDE.md and the parent CLAUDE.md). `cocode-rs/` is the
# read-only reference implementation and is intentionally not gated here.

LIGHT=0
for arg in "$@"; do
  case "$arg" in
    --light) LIGHT=1 ;;
    *) echo "rust-quality-check: unknown flag '$arg' (expected --light)" >&2; exit 2 ;;
  esac
done

# Best-effort observability trace: write one line per invocation so the
# agent can `cat /tmp/coco-stop-hook-trace.log` next turn to confirm the
# hook actually fired. Local time + offset so the timestamp is readable
# at a glance without UTC math; ISO 8601 format keeps it sortable.
# Failures (read-only fs, missing /tmp, etc.) are silently swallowed —
# never a reason to fail the gate.
{ echo "$(date "+%FT%T%z") rust-quality-check pid=$$ light=$LIGHT pwd=$PWD" \
    >> /tmp/coco-stop-hook-trace.log; } 2>/dev/null || true

# Ensure cargo/rustup are on PATH (hooks run without .bashrc).
[ -f "/usr/local/cargo/env" ] && . "/usr/local/cargo/env"
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

PROJECT_DIR="${CLAUDE_PROJECT_DIR:?CLAUDE_PROJECT_DIR not set}"
WORKSPACE="$PROJECT_DIR/coco-rs"

# Skip when nothing in coco-rs changed (Rust source OR Cargo manifest —
# manifest changes catch a vercel-ai-provider dep being re-introduced
# without touching any .rs file).
#
# `git status --porcelain` covers tracked modifications AND untracked
# files; bare `git diff HEAD` would miss new files (e.g. a fresh
# integration-test module) and silently skip the gate. Format is
# `XY <space> path` (rename: `R  old -> new` — we only need the new path,
# so strip everything up to the last space).
changed=$(git -C "$PROJECT_DIR" status --porcelain 2>/dev/null \
            | sed 's/^...//' \
            | sed 's/.* -> //' \
            | grep -E '^coco-rs/.*(\.rs|/Cargo\.toml)$' || true)
if [ -z "$changed" ]; then
  exit 0
fi

cd "$WORKSPACE"

# `CARGO_TARGET_DIR` deliberately unset — cargo's default workspace-relative
# `target/` already isolates worktrees. Override per-shell if you need the
# old `/tmp/cargo-target-<md5>` shim for virtiofs FD limits.

if [ "$LIGHT" -eq 1 ]; then
  echo "Rust changes detected — running quality checks (light: seam + fmt)..." >&2
else
  echo "Rust changes detected — running quality checks (seam + fmt + clippy)..." >&2
fi

# Step 0: vercel-ai seam guard — no direct vercel_ai_provider:: imports
# outside services/inference, no V4-suffixed type names leaking, no
# Cargo.toml depending on the seam crate outside the seam itself.
# Runs first because it's <1s and catches dep-graph violations that
# subsequent cargo steps would otherwise mask as "compiles fine".
if [ -x "./scripts/check-vercel-ai-seam.sh" ]; then
  SEAM_OUTPUT=$(./scripts/check-vercel-ai-seam.sh 2>&1) || {
    echo "vercel-ai seam violation:" >&2
    echo "$SEAM_OUTPUT" >&2
    echo "  → see services/inference/src/lib.rs for the canonical re-export list." >&2
    exit 2
  }
fi

# Step 1: fmt (auto-fix, should not fail).
just fmt 2>&1

# Step 2: clippy via shared script. `--incremental --head` lints
# {changed crates ∪ reverse-dep closure}; falls back to workspace clippy
# when Cargo.toml/Cargo.lock/toolchain change or affected ≥ 70%.
# Workspace policy is zero warnings — script exits non-zero on any warning.
# `just check` is intentionally NOT run here: clippy is a strict superset of
# check, and running both means rustc + clippy-driver compile every dep
# twice (different cache keys).
#
# Skipped in --light mode: the developer (or the next manual
# `just quick-check`) handles clippy. The Stop hook stays sub-second.
if [ "$LIGHT" -eq 1 ]; then
  echo "Quality checks passed (light)." >&2
  exit 0
fi

CLIPPY_OUTPUT=$(bash "$PROJECT_DIR/.claude/scripts/clippy.sh" --incremental --head 2>&1)
CLIPPY_RC=$?
if [ $CLIPPY_RC -ne 0 ]; then
  echo "Clippy warnings/errors detected. Fix them:" >&2
  echo "$CLIPPY_OUTPUT" >&2
  exit 2
fi

echo "Quality checks passed." >&2
exit 0
