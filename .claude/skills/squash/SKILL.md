---
allowed-tools: Bash(git:*)
description: Squash commits since a base commit with auto-generated message
argument-hint: <base-commit-id>
---

## Context

- Current branch: !`git branch --show-current`
- Git status: !`git status --short`
- Base commit: $ARGUMENTS

## Why

Collapse a branch's WIP history into one coherent commit before merge — easier
to review, revert, and cherry-pick than a noisy fixup series.

## What

Squash every commit in `$ARGUMENTS..HEAD` into a single commit.

- **Back up first.** Push the current branch to origin before resetting; the
  squash is locally destructive.
- **Don't drop uncommitted work.** Fold any staged or unstaged changes into the
  squash so nothing escapes.
- **Synthesize across the whole range** when writing the message — not from the
  tip commit alone.
- **Tree must be byte-identical** to the pre-squash range (same files, same
  insertions, same deletions).

## Commit message

Follow the project's `CLAUDE.md` Conventional Commits rules:

- **Subject:** `<type>(<scope>): <summary>` — imperative, ≤72 chars, no period.
  Types: `feat | fix | refactor | test | docs | chore | perf | ci | build | style | revert`.
- **Body:** 4–8 bullets, grouped by theme, each explaining *why*. No per-file
  recaps, no test counts, no rote "verified" lines.
- **Synthesize** — don't paste per-commit bodies.
- **Footers:** `BREAKING CHANGE:` and `Co-Authored-By:` only.
