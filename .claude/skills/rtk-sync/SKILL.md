---
allowed-tools: Bash, Read, Edit, AskUserQuestion
description: Full-chain update of the rtk fork (rebase onto upstream, resolve conflicts, verify lib) and integrate into coco-rs (rusqlite lockstep, rtk rev bump, pre-commit)
argument-hint: [check]
---

## What this does

End-to-end sync of the embedded **rtk** filter core into coco-rs:

1. Detect the diff between the fork `develop` and upstream `develop`. **If
   upstream has no new commits, skip — do nothing.**
2. Rebase the fork's delta onto the latest upstream; resolve conflicts.
3. Verify the fork's `lib` target still compiles — and, if upstream decoupled
   `cmds` from the binary-only `Commands` enum, opportunistically expose the
   git/cargo/pytest family formatters (`pub mod cmds`).
4. **Check the rusqlite version** and, if upstream moved it, update coco-rs to
   match (a hard `links` constraint — see below).
5. Force-push the rebased `develop` to the fork (confirmation-gated).
6. Bump coco-rs's `rtk` git dependency `rev` to the new SHA.
7. Run coco-rs `just quick-check` then `just pre-commit`.

`check` argument = **detect-and-report only** (step 1). No arg = the full chain.

## Execution model — script owns mechanics, you own judgment

Deterministic git/cargo plumbing lives in a bundled script; run it, don't
retype its commands:

```
SCRIPT="$(git rev-parse --show-toplevel)/.claude/skills/rtk-sync/rtk-sync.sh"
```

The script (`detect|rebase|verify|push`) bootstraps a **managed clone** (never a
dev checkout — those may hold unpushed commits or be dirty), reports a
`STATUS` block, and signals state via exit codes. **You** own everything that
needs judgment — conflict resolution, the rusqlite decision, editing coco — and
the two **confirm gates** (`push`, and committing coco changes). Never let the
script force-push or commit without a gate.

Exit codes: `0` ok · `3` up-to-date (skip) · `10` rebase conflict (paused) ·
`11` rebase error · `12` build failed · `64` usage.

## Fixed facts

| Thing | Value |
|---|---|
| Fork working clone | `$FORK` = `${COCO_RTK_SYNC_DIR:-$HOME/.cache/coco/rtk-fork-sync}`, created/reused by the script. **Never** a developer checkout. |
| Fork `origin` | `git@github.com:collab-ai-dev/rtk.git`, branch `develop` — the pushed **source of truth** for the delta |
| Upstream | `git@github.com:rtk-ai/rtk.git`, default branch `develop` |
| coco-rs workspace | `$COCO` = `$(git rev-parse --show-toplevel)/coco-rs`; run `just`/`cargo` here |
| coco rusqlite pin | `$COCO/Cargo.toml`: `rusqlite = { version = "0.31", … }` (lockstep comment above it) |
| coco rtk dep | search `$COCO` for `rtk = { git = "…collab-ai-dev/rtk", rev = "…" }` — **may not exist yet**; if absent, skip the rev bump |

## ⚠️ Load-bearing invariant: rusqlite `links` lockstep

`rusqlite` → `libsqlite3-sys`, which declares `links = "sqlite3"`. Cargo
**forbids two semver-incompatible versions of a `links` crate in one graph**.
The rtk lib pulls rusqlite (via `core::tracking`); coco pulls it (via
`coco-retrieval`). If the versions differ incompatibly, **coco fails to build**
(`multiple packages link to native library sqlite3`) — a hard error, not a slow
duplicate. So Step 4 is mandatory: coco's rusqlite MUST track the fork's.

---

## Step 1 — Detect (always)

Run `bash "$SCRIPT" detect`. It bootstraps the clone, fetches, and prints
`STATUS` (fork/coco dirs, `old_sha`, `upstream_ahead`, `fork_only`, both
rusqlite versions, and `rusqlite=MATCH|MISMATCH`). Parse it.

- **Exit 3 (`upstream_ahead=0`) → SKIP.** Report "already up to date". Then, even
  when skipping, if `rusqlite=MISMATCH` or coco's `rtk` `rev` ≠ `old_sha`,
  reconcile coco (Steps 4 & 6) — otherwise stop.
- Exit 0 → upstream has new commits; the script also printed them. Continue.
- **If the argument is `check`: stop here** and just report.

Sanity: `fork_only` should be your expected delta (the `src/lib.rs` commit).
The delta is read from `origin/develop` (pushed). If an expected commit is
missing it was never pushed — push it to the fork first, then re-run.

## Step 2 — Rebase (only if Step 1 said has_updates)

Run `bash "$SCRIPT" rebase` (backs up `backup/pre-rtk-sync`, rebases
`origin/develop`'s delta onto `upstream/develop`).

- Exit 0 → continue.
- **Exit 10 (conflict, rebase paused)** → resolve with judgment in `$FORK`, do
  not blind-accept a side. `src/lib.rs` is ours and additive; a real conflict
  there means upstream added its own `lib.rs` — reconcile so our `pub mod`
  surface survives. Any dep-gating patches we carry could conflict in
  `Cargo.toml` / `src/core/mod.rs` / `src/core/tracking.rs` — keep **both**
  sides. Then `git -C "$FORK" add <files>` and `git -C "$FORK" rebase --continue`
  (repeat). If genuinely ambiguous, ask the user. To bail:
  `git -C "$FORK" rebase --abort` (restores `origin/develop`) and report.
- Exit 11 → report and stop.

## Step 3 — Verify the lib builds

Run `bash "$SCRIPT" verify` (`cargo build --lib` in the clone; cold cache
compiles bundled sqlite — slow). Exit 12 → if upstream renamed a module our
`lib.rs` re-exports, fix `$FORK/src/lib.rs` (coco-owned, zero upstream-sync
cost) and re-run; if it broke for an unrelated upstream reason, report and stop.

## Step 3.5 — Expose `cmds` formatters if upstream decoupled them

The git/cargo/pytest family formatters live in `cmds`, which the lib omits while
`cmds` references `main.rs`'s binary-only clap `Commands` enum (upstream branch
`refacto/vitest-decouple` removes this). `STATUS` reports
`cmds_commands_coupling=present|absent`. Note: omitting `cmds` costs coco **no
functionality** — git/cargo/pytest still run and are captured; they just miss
rtk's in-process semantic compression (they fall back to head-truncation, i.e.
today's behavior). It is a missed optimization, not a missing feature.

- `present` → leave `cmds` omitted; note it and continue.
- `absent` → run `bash "$SCRIPT" try-cmds` (reversible — adds `pub mod cmds;` +
  auto-mirrored `pub use cmds::…` re-exports to `lib.rs`, builds, reverts on
  failure):
  - `result=cmds_exposed` → `lib.rs` now exports the family formatters; the fork
    delta grew but stays additive/coco-owned. This SHA (pushed in Step 5) is what
    unlocks git/cargo/pytest compression for coco.
  - `cmds_blocked` (exit 20) → a coupling remains; keep omitted, report it.
  - `cmds_build_failed` (exit 21) → reverted; report the blocker for manual
    follow-up (likely a new main.rs-root reference beyond `Commands`).

## Step 4 — rusqlite lockstep (the coco coupling)

Read `rusqlite=` from the `STATUS` block (or re-run `detect`).

- `MATCH` → nothing to do.
- `MISMATCH` (upstream moved rusqlite) → coco MUST follow (the `links`
  invariant):
  1. `Edit` `$COCO/Cargo.toml`: set `rusqlite = { version = "<new>", features = ["bundled"] }`; update the lockstep comment to the new version.
  2. `cargo update -p rusqlite --precise <new-x.y.z> --manifest-path "$COCO/Cargo.toml"`.
  3. `cargo build -p coco-retrieval --manifest-path "$COCO/Cargo.toml"` (its only user).
  4. If `coco-retrieval` breaks on the new API, stop and report — needs a code
     change, not just a bump.

## Step 5 — Push the fork (CONFIRM GATE)

The rebase rewrote `develop` → a `--force-with-lease` push to a shared branch:
outward-facing and destructive. **AskUserQuestion to confirm first**, showing the
fork-only commit(s), `old_sha`, and the new tip (`git -C "$FORK" rev-parse HEAD`).

On approval: `bash "$SCRIPT" push` → capture `NEW_SHA` from its output. If
declined: leave the rebase local, note `NEW_SHA` is unpushed (coco can't consume
it), and stop.

## Step 6 — Bump coco-rs's rtk rev

`grep -rn 'collab-ai-dev/rtk' "$COCO"`/*Cargo.toml*. If found: `Edit` `rev = "…"`
→ `NEW_SHA`, then `cargo update -p rtk --manifest-path "$COCO/Cargo.toml"`. If not
found, note the coco↔rtk dep isn't wired yet — nothing to bump; it'll pin
`rev = "<NEW_SHA>"` when first added.

## Step 7 — coco-rs checks

Per `$COCO/CLAUDE.md`, from `$COCO`: `just quick-check`; if green, the final gate
**once**: `just pre-commit`. Don't run `pre-commit` before `quick-check` is
clean. Report any real breakage with its output — don't paper over it.

## Step 8 — Report & optional commit

Summarize: upstream commits pulled, conflicts + resolution, rusqlite decision,
`old_sha → NEW_SHA`, whether coco's `rev` moved, check results.

Working dir: with the default cache dir, leave `$FORK` for the next run's warm
reuse; a `mktemp -d` clone should be removed on success, kept + path-reported on
failure.

If coco files changed and checks are green, **offer** to commit (separate
confirm — never auto-commit), e.g.
`chore(rtk): sync fork to upstream develop <short-sha>, bump rusqlite to X`,
ending the body with the repo's required `Co-Authored-By` trailer.

---

## Notes & rationale

- **Skip when upstream is unchanged.** `detect` exits 3 when
  `upstream_ahead == 0`; the whole rebase/push/coco chain is bypassed. Re-running
  after a successful sync is a no-op (origin/develop already contains upstream).
- **Why a managed clone, not a dev checkout?** Must run on any machine/user, and
  a local clone may hold unpushed commits (e.g. `84d8564`) or be dirty — never
  trusted. Bootstrapped from `origin/develop` into a cache dir, reused for warm
  git + build caches, hard-reset to `origin/develop` each run. coco consumes the
  fork by pushed git rev anyway, so the clone is pure scratch. Rollback =
  `rebase --abort` or `git -C "$FORK" reset --hard origin/develop`. Override the
  location with `COCO_RTK_SYNC_DIR`; use `mktemp -d` for a throwaway.
- **Why bash, not python?** Pure git/cargo orchestration, no data processing —
  bash needs zero runtime deps and matches the repo's other shell git skills.
- **Why rebase, not merge?** Keeps `develop` = "upstream + our small additive
  delta", linear. Cost is the gated force-push. Non-destructive alternative:
  `git -C "$FORK" merge upstream/develop`.
- Related: `docs/coco-rs/rtk-integration-design.md` §3.2; memory
  `rtk-fork-lib-target`.
