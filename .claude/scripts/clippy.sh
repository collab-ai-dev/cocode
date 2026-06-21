#!/usr/bin/env bash
# .claude/scripts/clippy.sh
#
# Unified clippy gate for coco-rs/. Single source of truth for:
#   - just clippy           (--full)
#   - just clippy-affected  (--incremental --head)
#   - .claude Stop hook     (--incremental --head)
#   - git pre-commit hook   (--incremental --staged)
#
# Incremental mode lints {changed crates ∪ transitive reverse-dep closure},
# so a change in crate A that breaks crate B (which depends on A) is still
# caught. Falls back to full workspace clippy when:
#   - any Cargo.toml or Cargo.lock changed
#   - rust-toolchain.toml or .cargo/config changed
#   - affected crate count >= COCO_CLIPPY_FALLBACK_PCT (default 70) of workspace
#   - cargo metadata fails (broken manifest)
#
# Output: status to stderr; clippy noise printed to stderr only on failure.
# Workspace policy is zero warnings — script exits non-zero on any warning.
#
# Exit:
#   0 — pass
#   1 — clippy errors or warnings (output already on stderr)
#   2 — usage / setup error

set -uo pipefail

# ── Argument parsing ────────────────────────────────────────────────
MODE="full"
DIFF_SOURCE="head"
DIFF_BASE=""
EXTRA_ARGS=()

while [ $# -gt 0 ]; do
  case "$1" in
    --full)         MODE="full"; shift ;;
    --incremental)  MODE="incremental"; shift ;;
    --head)         DIFF_SOURCE="head"; shift ;;
    --staged)       DIFF_SOURCE="staged"; shift ;;
    --base)         DIFF_SOURCE="base"; DIFF_BASE="${2:-}"; shift 2 ;;
    --) shift; EXTRA_ARGS+=("$@"); break ;;
    *)  EXTRA_ARGS+=("$1"); shift ;;
  esac
done

# ── Locate workspace ────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="${CLAUDE_PROJECT_DIR:-$(cd "$SCRIPT_DIR/../.." && pwd)}"
WORKSPACE_DIR="$PROJECT_DIR/coco-rs"

if [ ! -f "$WORKSPACE_DIR/Cargo.toml" ]; then
  echo "clippy.sh: workspace not found at $WORKSPACE_DIR" >&2
  exit 2
fi

# Ensure cargo on PATH (hooks may run without a login shell).
[ -f "/usr/local/cargo/env" ] && . "/usr/local/cargo/env"
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

cd "$WORKSPACE_DIR"

# ── Run clippy with capture-on-failure ──────────────────────────────
run_clippy() {
  local args=("$@")
  echo "clippy.sh: cargo clippy ${args[*]}" >&2

  local output rc warns
  output=$(cargo clippy "${args[@]}" 2>&1)
  rc=$?
  warns=$(echo "$output" | grep '^warning:' | grep -v 'generated\|warnings emitted' || true)

  if [ $rc -ne 0 ] || [ -n "$warns" ]; then
    echo "$output" >&2
    if [ $rc -ne 0 ]; then
      echo "clippy.sh: clippy errored (exit $rc)" >&2
    else
      echo "clippy.sh: clippy emitted warnings (workspace policy: zero warnings)" >&2
    fi
    exit 1
  fi
}

run_full() {
  run_clippy --workspace --all-features --tests ${EXTRA_ARGS[@]+"${EXTRA_ARGS[@]}"}
  echo "clippy.sh: passed (workspace)" >&2
  exit 0
}

# ── Full mode ───────────────────────────────────────────────────────
if [ "$MODE" = "full" ]; then
  run_full
fi

# ── Incremental mode ────────────────────────────────────────────────

# Step 1: changed files (relative to repo root) limited to coco-rs/
#
# `git diff` only reports tracked files; brand-new files that haven't
# been `git add`ed yet are invisible to it. `head` mode is what the Stop
# hook and `quick-check` use, so we also fold in `ls-files --others
# --exclude-standard` (untracked, gitignore-aware) so a fresh `.rs` test
# file authored mid-iteration is still linted instead of silently skipped.
# `staged` mode keeps the existing semantics — untracked is not staged.
case "$DIFF_SOURCE" in
  head)
    diffed=$(git -C "$PROJECT_DIR" diff --name-only --diff-filter=ACMR HEAD 2>/dev/null \
                | grep -E '^coco-rs/' || true)
    untracked=$(git -C "$PROJECT_DIR" ls-files --others --exclude-standard 2>/dev/null \
                | grep -E '^coco-rs/' || true)
    all_changed=$(printf '%s\n%s' "$diffed" "$untracked" | awk 'NF' | sort -u) ;;
  staged)
    all_changed=$(git -C "$PROJECT_DIR" diff --cached --name-only --diff-filter=ACMR 2>/dev/null \
                    | grep -E '^coco-rs/' || true) ;;
  base)
    if [ -z "$DIFF_BASE" ]; then
      echo "clippy.sh: --base requires a ref argument" >&2
      exit 2
    fi
    all_changed=$(git -C "$PROJECT_DIR" diff --name-only --diff-filter=ACMR "$DIFF_BASE"...HEAD 2>/dev/null \
                    | grep -E '^coco-rs/' || true) ;;
esac

if [ -z "$all_changed" ]; then
  echo "clippy.sh: no coco-rs changes — skipping" >&2
  exit 0
fi

# Step 2: full-mode triggers
trigger=""
if echo "$all_changed" | grep -q '^coco-rs/Cargo\.lock$'; then
  trigger="Cargo.lock"
elif echo "$all_changed" | grep -q '^coco-rs/rust-toolchain\.toml$'; then
  trigger="rust-toolchain.toml"
elif echo "$all_changed" | grep -q '^coco-rs/\.cargo/'; then
  trigger=".cargo/config"
elif echo "$all_changed" | grep -q '/Cargo\.toml$'; then
  trigger="Cargo.toml"
fi

if [ -n "$trigger" ]; then
  echo "clippy.sh: $trigger changed → falling back to full workspace clippy" >&2
  run_full
fi

# Step 3: only Rust source files matter for crate mapping
changed_rs=$(echo "$all_changed" | grep -E '\.rs$' || true)
if [ -z "$changed_rs" ]; then
  echo "clippy.sh: no .rs changes — skipping" >&2
  exit 0
fi

# Step 4: build {crate-dir → pkg-name} map from cargo metadata. Sort by prefix
# length descending so longest-prefix match wins (handles nested workspaces
# correctly).
metadata=$(cargo metadata --no-deps --format-version 1 2>/dev/null) || {
  echo "clippy.sh: cargo metadata failed → falling back to full workspace clippy" >&2
  run_full
}

PKG_MAP=$(
  echo "$metadata" \
    | jq -r '.packages[] | "\(.manifest_path | sub("/Cargo.toml$"; ""))\t\(.name)"' \
    | awk -F'\t' '{print length($1)"\t"$0}' \
    | sort -rn \
    | cut -f2-
)

file_to_pkg() {
  local file="$1"
  local abs="$PROJECT_DIR/$file"
  local entry prefix name
  while IFS= read -r entry; do
    [ -z "$entry" ] && continue
    prefix="${entry%%$'\t'*}"
    name="${entry#*$'\t'}"
    case "$abs" in
      "$prefix"/*|"$prefix") echo "$name"; return ;;
    esac
  done <<< "$PKG_MAP"
}

set_add() {
  local set="$1"
  local value="$2"
  local item
  [ -z "$value" ] && { printf '%s' "$set"; return; }
  while IFS= read -r item; do
    [ "$item" = "$value" ] && { printf '%s' "$set"; return; }
  done <<< "$set"
  if [ -n "$set" ]; then
    printf '%s\n%s' "$set" "$value"
  else
    printf '%s' "$value"
  fi
}

set_count() {
  local set="$1"
  if [ -z "$set" ]; then
    echo 0
  else
    printf '%s\n' "$set" | awk 'NF { n++ } END { print n + 0 }'
  fi
}

# Step 5: changed crates
changed_pkgs=""
while IFS= read -r file; do
  [ -z "$file" ] && continue
  pkg=$(file_to_pkg "$file")
  changed_pkgs=$(set_add "$changed_pkgs" "$pkg")
done <<< "$changed_rs"

changed_count=$(set_count "$changed_pkgs")
if [ "$changed_count" -eq 0 ]; then
  echo "clippy.sh: no .rs changes mapped to a workspace crate — skipping" >&2
  exit 0
fi

# Step 6: reverse-dep closure (full edges; includes dev-deps to catch
# test-only consumers like coco-test-harness).
affected=""
while IFS= read -r pkg; do
  [ -z "$pkg" ] && continue
  affected=$(set_add "$affected" "$pkg")
  while IFS= read -r dep; do
    affected=$(set_add "$affected" "$dep")
  done < <(cargo tree -i "$pkg" --prefix none --all-features 2>/dev/null \
             | awk 'NF{print $1}' | sort -u)
done <<< "$changed_pkgs"

# Step 7: threshold check
total=$(echo "$metadata" | jq '.packages | length')
threshold_pct="${COCO_CLIPPY_FALLBACK_PCT:-70}"
threshold=$((total * threshold_pct / 100))
affected_count=$(set_count "$affected")

echo "clippy.sh: changed=$changed_count → affected=$affected_count / $total (threshold=$threshold @ ${threshold_pct}%)" >&2

if [ "$affected_count" -ge "$threshold" ]; then
  echo "clippy.sh: affected ≥ ${threshold_pct}% → falling back to full workspace clippy" >&2
  run_full
fi

# Step 8: scoped clippy — single invocation with -p flags
pkg_args=()
while IFS= read -r pkg; do
  [ -z "$pkg" ] && continue
  pkg_args+=("-p" "$pkg")
done <<< "$affected"

echo "clippy.sh: scoped to: $(printf '%s\n' "$affected" | tr '\n' ' ')" >&2
run_clippy "${pkg_args[@]}" --all-features --tests ${EXTRA_ARGS[@]+"${EXTRA_ARGS[@]}"}
echo "clippy.sh: passed (incremental, $affected_count/$total pkgs)" >&2
exit 0
