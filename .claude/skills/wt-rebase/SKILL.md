---
allowed-tools: Bash(git:*), Read, Edit, AskUserQuestion
description: Rebase current branch onto a target branch (linear history, no merge commit) — keeps GitHub "Rebase and merge" clean
argument-hint: <onto-branch>
---

## Context

- Current branch: !`git branch --show-current`
- Git status: !`git status --short`
- Target base branch (rebase ONTO): $ARGUMENTS
- Available branches: !`git branch --list`

## Task

Rebase the **current branch** onto `$ARGUMENTS`: take the commits unique to the
current branch and replay them on top of `$ARGUMENTS`. The result is a linear
history with **no merge commit**, so GitHub "Rebase and merge" can flatten and
replay the PR cleanly without re-hitting conflicts.

Use this instead of `/wt-merge` when the two branches have diverged but you want
to keep the branch mergeable via GitHub's rebase strategy.

> Direction matters: `/wt-rebase feat/core` means "move my current branch on top
> of feat/core" — the same integration direction as `/wt-merge feat/core`, but
> via replay instead of a merge commit.

### Step 1: Validate Input

1. Check that a branch name is provided in `$ARGUMENTS`
2. If empty, report error: "Usage: /wt-rebase <onto-branch>"
3. Verify the branch exists: `git rev-parse --verify $ARGUMENTS`
4. Refuse if `$ARGUMENTS` equals the current branch (nothing to rebase onto)
5. Confirm you are on a branch, not a detached HEAD: `git symbolic-ref -q HEAD`.
   If detached, refuse — the rebase result would be left unreferenced.

### Step 2: Pre-rebase Check

1. Check for uncommitted changes in the current branch. If dirty, ask the user
   whether to:
   - Stash changes before the rebase (will `git stash pop` afterwards; warn that
     the pop can itself conflict — if it does, the stash stays in `git stash list`
     until resolved and `git stash drop`-ed)
   - Commit changes first
   - Abort
2. Determine the relationship and short-circuit the trivial cases:
   - `git merge-base --is-ancestor $ARGUMENTS HEAD` → target is **already in
     history**. Report "Already up to date — current branch is linearly on top of
     `$ARGUMENTS`, nothing to rebase." and stop.
   - `git merge-base --is-ancestor HEAD $ARGUMENTS` → current branch is behind
     (the equal case is already handled by the check above); the rebase is a
     trivial fast-forward with no commits to replay. Proceed.
3. Show what will happen:
   - Commits to be replayed (yours): `git log $ARGUMENTS..HEAD --oneline`
   - Incoming base commits you'll land on: `git log HEAD..$ARGUMENTS --oneline`

### Step 3: Execute Rebase

1. Run: `git rebase $ARGUMENTS`
2. Check the result — success, fast-forward, or stopped on a conflict.

### Step 4: Handle Conflicts (if any)

A rebase replays your commits **one at a time**, so conflicts can recur once per
replayed commit. Treat this as a loop, not a single resolution.

> ⚠️ **Conflict sides are INVERTED vs merge.** During a rebase, git checks out
> the base (`$ARGUMENTS` + already-replayed commits) and applies your commit on
> top. So at a conflict:
> - **`ours` / HEAD side = the base branch** (`$ARGUMENTS` and what's replayed so far)
> - **`theirs` side = the commit of yours currently being replayed**
>
> This is the opposite of `/wt-merge`. Do not mix them up when choosing a side.

For each conflict round:

1. List conflicted files: `git diff --name-only --diff-filter=U`
2. For each conflicted file:
   - Read the file to understand the conflict
   - Analyze both sides — `ours` = base, `theirs` = your replayed commit
   - Resolve when the resolution is clear:
     - Simple additions from both sides → combine them
     - Same change on both sides → keep one
   - For complex conflicts, show the conflict and ask the user for a preference
3. **Special handling for `.claude/settings.local.json`**:
   - When reconciling `permissions.allow` entries, only keep **generic, reusable** entries
   - **Remove** entries that are:
     - Bound to specific local/absolute paths (e.g. `Bash(find /lyz/codespace/...)`, `Read(//lyz/codespace/worktrees/...)`)
     - Bound to specific temp files or session paths (e.g. `Read(//tmp/**)`, `Bash(/tmp/...)`)
     - Shell loop fragments that aren't standalone commands (e.g. `Bash(done)`, `Bash(do echo:*)`, `Bash(do sleep:*)`)
     - Accidental or meaningless entries (e.g. `Bash(2)`)
   - **Keep** entries that are:
     - Tool/command wildcards (e.g. `Bash(cargo:*)`, `Bash(git:*)`)
     - Domain-scoped WebFetch (e.g. `WebFetch(domain:docs.rs)`)
     - Skill invocations (e.g. `Skill(update-claude-md)`)
     - Generic system commands (e.g. `Bash(dpkg -l)`, `Bash(apt list:*)`)
4. After resolving the current round:
   - Stage resolved files: `git add <file>`
   - Continue the rebase: `git rebase --continue`
   - **Do NOT run `git commit`** — rebase reuses each commit's original message. If
     an editor opens for the message, keep it unchanged.
   - Repeat from step 1 if the next replayed commit also conflicts.
5. If a conflict cannot be resolved (or the user wants out), run
   `git rebase --abort` — this restores the branch exactly to its pre-rebase
   state. Report that the rebase was aborted with no changes.

### Step 5: Report Result

Report:
- Rebase status (success / fast-forward / conflicts resolved / aborted)
- Number of commits replayed
- The resulting linear history: `git log $ARGUMENTS..HEAD --oneline`
- Files changed: `git diff --stat $ARGUMENTS..HEAD` (double-dot — tree diff of
  HEAD vs the base, consistent with the log range above)
- Any conflicts that were resolved and how (note which side was `ours` vs `theirs`)
- **Push note**: the rebase rewrote the current branch's commit SHAs. If this
  branch was already pushed, the next push needs `git push --force-with-lease`.
  **Do not force-push automatically** — tell the user and let them run it.
- **Recovery note**: the pre-rebase tip is saved in `ORIG_HEAD` — undo the whole
  rebase with `git reset --hard ORIG_HEAD`. In `git reflog` it is the entry **just
  before** `rebase (start)` (your last real `commit:` / `checkout:` line). Do
  **not** reset to the `rebase (start)` entry itself — that points at the new base
  `$ARGUMENTS`, and resetting there would discard your replayed commits.
