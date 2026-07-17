---
allowed-tools: Bash(git:*)
description: Squash commits since a base into one Conventional Commit
argument-hint: "[base] — omit to use origin's default branch"
---

## Context

- Current branch: !`git branch --show-current`
- Git status: !`git status --short`
- Requested base: $ARGUMENTS

## Why

Collapse a branch's WIP history into one coherent commit before merge — easier
to review, revert, and cherry-pick than a noisy fixup series.

## Resolve the base

`$ARGUMENTS` may be empty, a commit-ish (`abc1234`, `origin/main`), or a range
(`origin/main..HEAD`). Normalize it — never interpolate it raw:

- **Empty** → `git fetch origin` (best-effort: warn and continue if offline; a
  stale base still squashes correctly), then use `origin/main`. This project
  only has `main`.
- **Range** → strip the `..HEAD` suffix, keep the left side. `origin/main` and
  `origin/main..HEAD` must mean the same thing; appending `..HEAD` blindly
  builds `origin/main..HEAD..HEAD`, which is not a range.
- **Commit-ish** → use as-is.

## Reset to the MERGE-BASE, never to the base ref itself

Squash only the commits **after the fork point** — the node the branch and
`main` share. Never let `main`'s own commits enter the branch:

```sh
BASE=$(git merge-base <resolved> HEAD)   # the fork point
git reset --soft "$BASE" && git commit
```

**This is not optional.** When the branch is *behind* the base — upstream moved
while you worked — `git reset --soft origin/main` hands the new commit a parent
it was never built on. The tree stays yours, so the resulting diff silently
**reverts every upstream commit you lacked**. It then passes review as "one
clean commit" while quietly deleting someone else's work. Rebasing onto `main`
is a separate decision the user makes explicitly; a squash must never smuggle it
in.

`<resolved>..HEAD` is still the correct range for *listing* what you're about to
squash — two-dot ranges are already merge-base-relative. That is precisely why
the range form looks safe and the reset form is not: only the reset target must
be the explicit fork point.

## Rules

- **Back up locally first:** `git branch -f <branch>-backup HEAD` (the current
  branch name plus a `-backup` suffix, so the backup sorts next to its source).
  A local ref is enough to recover from a bad squash and, unlike a push, does
  not publish WIP or trigger CI. Push only when the user asks for off-machine
  backup — that's an outward-facing action, not a backup detail.
- **Don't drop uncommitted work.** Fold staged and unstaged changes into the
  squash so nothing escapes.
- **Refuse to squash nothing.** If the range is empty — or is a single commit
  with a clean working tree — say so and stop rather than rewrite history for
  no gain.
- **Synthesize across the whole range** when writing the message — not from the
  tip commit alone.
- **Verify before reporting success** — always, as the closing step:
  - `git rev-parse HEAD^{tree}` equals the tree recorded before resetting.
  - `git diff <branch>-backup HEAD` is empty.
  - `git log --oneline <resolved>..HEAD` shows exactly one commit.
  - `git merge-base --is-ancestor <resolved> HEAD` still **fails** if it failed
    before — i.e. the squash did not quietly pull `main` in.

  A squash that changes the tree is a bug, not a squash. Report the checks, not
  just the new hash.

## Commit message

Follow the project's `CLAUDE.md` Conventional Commits rules:

- **Subject:** `<type>(<scope>): <summary>` — imperative, ≤72 chars, no period.
  Types: `feat | fix | refactor | test | docs | chore | perf | ci | build | style | revert`.
- **Body:** 4–8 bullets, grouped by theme, each explaining *why*. No per-file
  recaps, no test counts, no rote "verified" lines.
- **Synthesize** — don't paste per-commit bodies.
- **Footers:** `BREAKING CHANGE:` and `Co-Authored-By:` only.
