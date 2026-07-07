#!/usr/bin/env bash
# rtk-sync.sh — deterministic mechanics for the /rtk-sync skill.
#
# Split of responsibility:
#   - THIS SCRIPT owns reproducible git/cargo plumbing + state detection and
#     reports machine-readable STATUS. It NEVER force-pushes or commits on its
#     own initiative except via the explicit `push` subcommand.
#   - THE AGENT owns judgment (conflict resolution, the rusqlite version
#     decision, editing coco-rs) and the confirm gates (calling `push`, and
#     committing coco changes).
#
# Subcommands & exit codes:
#   detect  -> bootstrap clone, fetch, print STATUS.
#                exit 0  = upstream has new commits (proceed)
#                exit 3  = up to date, NOTHING TO DO (skip the whole chain)
#   rebase  -> backup ref + rebase origin/develop's delta onto upstream/develop.
#                exit 0  = clean; exit 10 = conflict (paused, agent resolves);
#                exit 11 = other rebase error
#   verify  -> cargo build --lib in the clone.  exit 0 ok; exit 12 build failed
#   try-cmds-> if cmds is decoupled from the binary-only Commands enum, add
#                `pub mod cmds` (+ mirrored re-exports) to lib.rs and build.
#                exit 0 = cmds exposed; 20 = still coupled (skip); 21 = build
#                failed (reverted).  Run AFTER rebase (needs upstream's cmds).
#   push    -> git push origin develop --force-with-lease (agent calls AFTER a
#                confirm gate).  exit 0 = pushed
#   help    -> usage.  Unknown subcommand -> exit 64.
set -euo pipefail

FORK_URL="git@github.com:collab-ai-dev/rtk.git"
UPSTREAM_URL="git@github.com:rtk-ai/rtk.git"
BRANCH="develop"

# Managed working clone — never a developer checkout. Override with the env var.
FORK="${COCO_RTK_SYNC_DIR:-$HOME/.cache/coco/rtk-fork-sync}"

log() { printf '%s\n' "$*" >&2; }
g()   { git -C "$FORK" "$@"; }

coco_dir() {
  local top
  top="$(git rev-parse --show-toplevel 2>/dev/null || true)"
  if [ -n "$top" ] && [ -f "$top/coco-rs/Cargo.toml" ]; then
    echo "$top/coco-rs"
  elif [ -f "$PWD/coco-rs/Cargo.toml" ]; then
    echo "$PWD/coco-rs"
  else
    echo ""
  fi
}

rusqlite_ver() {  # $1 = Cargo.toml path -> prints "X.Y[.Z]" or nothing
  grep -m1 -E '^[[:space:]]*rusqlite[[:space:]]*=' "$1" 2>/dev/null \
    | grep -oE '[0-9]+\.[0-9]+(\.[0-9]+)?' | head -1 || true
}

# clone-if-absent, reuse-if-present, hard-clean to origin/develop, wire upstream
bootstrap() {
  if [ -d "$FORK/.git" ] && g remote get-url origin 2>/dev/null | grep -q 'collab-ai-dev/rtk'; then
    log "reusing managed clone: $FORK"
    g rebase --abort >/dev/null 2>&1 || true
    g fetch origin --prune -q
    g switch -f "$BRANCH"
    g reset --hard "origin/$BRANCH"
  else
    log "fresh clone -> $FORK"
    rm -rf "$FORK"
    mkdir -p "$(dirname "$FORK")"
    git clone -q "$FORK_URL" "$FORK"
  fi
  g remote get-url upstream >/dev/null 2>&1 || g remote add upstream "$UPSTREAM_URL"
  # only the branch we rebase onto, no tags — keeps output quiet and fetch fast
  g fetch upstream "$BRANCH" --no-tags -q
}

print_status() {
  local coco fork_rq coco_rq verdict
  coco="$(coco_dir)"
  fork_rq="$(rusqlite_ver "$FORK/Cargo.toml")"
  coco_rq=""
  [ -n "$coco" ] && coco_rq="$(rusqlite_ver "$coco/Cargo.toml")"
  verdict="UNKNOWN"
  if [ -n "$fork_rq" ] && [ -n "$coco_rq" ]; then
    if [ "${fork_rq%.*}" = "${coco_rq%.*}" ] || [ "$fork_rq" = "$coco_rq" ]; then
      verdict="MATCH"
    else
      verdict="MISMATCH"
    fi
  fi
  echo "STATUS"
  echo "fork_dir=$FORK"
  echo "coco_dir=${coco:-<not found>}"
  echo "old_sha=$(g rev-parse "origin/$BRANCH")"
  echo "upstream_ahead=$(g rev-list --count "origin/$BRANCH..upstream/$BRANCH")"
  echo "fork_only=$(g rev-list --count "upstream/$BRANCH..origin/$BRANCH")"
  echo "fork_rusqlite=${fork_rq:-?}"
  echo "coco_rusqlite=${coco_rq:-?}"
  echo "rusqlite=$verdict"
  # Can the cmds family formatters (git/cargo/pytest) be exposed in the lib?
  # Blocked while cmds references main.rs's binary-only clap `Commands` enum
  # (upstream branch refacto/vitest-decouple removes this). Checked on
  # upstream/develop so `detect`/`check` reports it before any rebase.
  local cmds_coupling=absent
  if g grep -q 'crate::Commands' "upstream/$BRANCH" -- src/cmds 2>/dev/null; then
    cmds_coupling=present
  fi
  echo "cmds_commands_coupling=$cmds_coupling"
}

cmd_detect() {
  bootstrap
  print_status
  local ahead
  ahead="$(g rev-list --count "origin/$BRANCH..upstream/$BRANCH")"
  echo "---"
  if [ "$ahead" -eq 0 ]; then
    log "upstream has NO new commits over origin/$BRANCH — skip: nothing to rebase."
    echo "result=up_to_date"
    exit 3
  fi
  log "upstream is ahead by $ahead commit(s):"
  g log --oneline "origin/$BRANCH..upstream/$BRANCH" >&2
  echo "result=has_updates"
}

cmd_rebase() {
  g switch -f "$BRANCH"
  g reset --hard "origin/$BRANCH"
  g branch -f backup/pre-rtk-sync "origin/$BRANCH"
  if g rebase "upstream/$BRANCH"; then
    echo "result=rebased_clean"
    echo "new_sha=$(g rev-parse HEAD)"
  else
    if [ -d "$FORK/.git/rebase-merge" ] || [ -d "$FORK/.git/rebase-apply" ]; then
      log "CONFLICT — rebase paused. Resolve in $FORK, 'git -C $FORK add <files>',"
      log "then 'git -C $FORK rebase --continue' (repeat), then re-run: verify."
      g status --short >&2 || true
      echo "result=conflict"
      exit 10
    fi
    log "rebase failed (non-conflict). Inspect $FORK."
    echo "result=error"
    exit 11
  fi
}

cmd_verify() {
  log "building lib target (cold cache compiles bundled sqlite — slow)…"
  if cargo build --lib --manifest-path "$FORK/Cargo.toml"; then
    echo "result=build_ok"
    echo "new_sha=$(g rev-parse HEAD)"
  else
    echo "result=build_failed"
    exit 12
  fi
}

cmd_push() {  # agent calls ONLY after an explicit confirm gate
  g push origin "$BRANCH" --force-with-lease
  echo "result=pushed"
  echo "new_sha=$(g rev-parse HEAD)"
}

# Opportunistically expose the cmds family formatters once upstream removed the
# `crate::Commands` coupling. Fully reversible: reverts lib.rs if it won't build.
cmd_try_cmds() {
  local blockers backup
  blockers="$(g grep -n 'crate::Commands' -- src/cmds 2>/dev/null || true)"
  if [ -n "$blockers" ]; then
    log "cmds still references the binary-only Commands enum — cannot expose yet:"
    printf '%s\n' "$blockers" >&2
    echo "result=cmds_blocked"
    exit 20
  fi
  if grep -q '^pub mod cmds;' "$FORK/src/lib.rs"; then
    log "lib.rs already exposes cmds."
    echo "result=cmds_exposed"
    return 0
  fi
  backup="$(mktemp)"
  cp "$FORK/src/lib.rs" "$backup"
  {
    echo ""
    echo "// cmds family formatters (git/cargo/pytest/…), exposed because upstream"
    echo "// decoupled cmds from the binary-only clap \`Commands\` enum. The re-exports"
    echo "// mirror main.rs's crate-root \`use cmds::…\` lines (auto-extracted, drift-proof)"
    echo "// so intra-crate \`crate::<x>\` paths resolve in the lib target."
    echo "pub mod cmds;"
    grep -E '^use cmds::' "$FORK/src/main.rs" | sed 's/^use /pub use /'
  } >> "$FORK/src/lib.rs"
  if cargo build --lib --manifest-path "$FORK/Cargo.toml"; then
    rm -f "$backup"
    echo "result=cmds_exposed"
    echo "new_sha=$(g rev-parse HEAD 2>/dev/null || true)"
  else
    cp "$backup" "$FORK/src/lib.rs"
    rm -f "$backup"
    log "adding \`pub mod cmds\` did not compile — reverted lib.rs (see errors above)."
    echo "result=cmds_build_failed"
    exit 21
  fi
}

case "${1:-help}" in
  detect)   cmd_detect ;;
  rebase)   cmd_rebase ;;
  verify)   cmd_verify ;;
  try-cmds) cmd_try_cmds ;;
  push)     cmd_push   ;;
  help|-h|--help)
    grep -E '^#( |$)' "$0" | sed 's/^# \{0,1\}//'
    ;;
  *) log "unknown subcommand: $1"; log "usage: rtk-sync.sh {detect|rebase|verify|push|help}"; exit 64 ;;
esac
