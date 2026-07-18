---
allowed-tools: Bash(git:*), Bash(rm:*)
description: Delete a worktree and its associated local branch
argument-hint: <branch-name>
---

## Context

- Current branch: !`git branch --show-current`
- Working directory: !`pwd`
- Existing worktrees: !`git worktree list`
- Branch to delete: $ARGUMENTS

## Task

Delete the worktree for branch `$ARGUMENTS` and remove the associated local branch.

### Step 1: Validate Input

1. Check that branch name is provided in `$ARGUMENTS`
2. If empty, report error: "Usage: /wt-del <branch-name>"
3. Verify we are NOT currently in the worktree being deleted

### Step 2: Locate Worktree

1. Get project name: `PROJECT=$(basename "$(pwd)")`
2. Convert branch name to safe directory name: replace `/` with `-`
3. Calculate expected worktree path: `../worktrees/${PROJECT}-${SAFE_BRANCH}`
4. Verify worktree exists at that path or find it via `git worktree list`

### Step 3: Remove Worktree

1. Remove the worktree: `git worktree remove <path>`
2. If worktree has uncommitted changes:
   - Report the situation
   - Ask user whether to force remove: `git worktree remove --force <path>`
3. Prune stale worktree info: `git worktree prune`

### Step 4: Check Integration Status (local `main` + `origin/main`)

`git branch -d` only tests the *local* `main`/upstream, so it wrongly reports
"not fully merged" for a branch that was already **squash-merged** into
`origin/main` via a PR (the squash commit has a different hash). Determine real
integration before deciding how to delete.

1. Refresh the remote (best-effort — continue on failure, e.g. offline / no
   remote): `git fetch origin main` (updates the `origin/main` tracking ref).
   If it fails, note it and fall back to checking local `main` only.

2. Build the list of integration refs to test — whichever of `main` (local) and
   `origin/main` exist.

3. For each integration ref `REF`, test whether `$ARGUMENTS` is already in it.
   The branch counts as **integrated** if *any* ref passes *either* test:
   - **Direct / rebase / fast-forward merge** (commit is present as-is):
     `git merge-base --is-ancestor $ARGUMENTS REF` (exit 0 = every branch commit
     is already reachable from `REF`).
   - **Squash merge** (patch-equivalent, different hash): if the ancestor test
     fails, squash the branch to one commit and ask whether `REF` already
     contains that combined patch:
     ```bash
     MB=$(git merge-base "$ARGUMENTS" REF)
     SQUASH=$(git commit-tree "$ARGUMENTS^{tree}" -p "$MB" -m _)
     git cherry REF "$SQUASH" | grep -q '^-'   # a '-' line => patch already in REF
     ```

### Step 5: Delete Local Branch

- **Integrated** (safe — no work lost): delete with `git branch -D $ARGUMENTS`
  (use `-D`; `-d` does not recognize squash-merges, and safety is already
  proven). Report which ref it was integrated into (e.g. "already in
  `origin/main`").
- **NOT integrated** (branch has unique, unmerged work): do **NOT** auto
  force-delete.
  1. List exactly what would be lost — commits on the branch not on any
     integration ref: `git log --oneline $ARGUMENTS --not main origin/main`.
  2. Ask the user to confirm force deletion.
  3. Only on explicit confirmation: `git branch -D $ARGUMENTS`.

### Step 6: Report Result

Report:
- Worktree path removed
- Branch deleted, and its integration status (which ref it was merged into, or
  that it was force-deleted with unmerged commits)
- Remaining worktrees: `git worktree list`
