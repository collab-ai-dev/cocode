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

`git branch -d` only tests the *local* `main`/upstream, and a merged PR lands on
`origin/main` with **different commit hashes** than the branch:

- **Merge-commit / fast-forward** keeps the original hashes.
- **Squash-merge** collapses the whole branch into one new commit.
- **Rebase-merge** re-applies each branch commit as a new, separate commit.

So every hash-based check (`-d`, `--is-ancestor`, `git log --not`) wrongly
reports "not merged" for squash- and rebase-merged branches. Decide integration
by **patch content**, not by hash.

1. Refresh the remote (best-effort — continue on failure, e.g. offline / no
   remote): `git fetch origin main` (updates the `origin/main` tracking ref).
   If it fails, note it and fall back to checking local `main` only.

2. Build the list of integration refs to test — whichever of `main` (local) and
   `origin/main` exist.

3. For each integration ref `REF`, the branch counts as **integrated** if ANY
   test passes. Run them in order and stop at the first hit:
   - **(a) Merge-commit / fast-forward** — commits present as-is:
     `git merge-base --is-ancestor $ARGUMENTS REF` (exit 0 = every branch commit
     is already reachable from `REF`).
   - **(b) Rebase-merge / cherry-pick** — each commit re-applied under a new
     hash. `git cherry` compares by *patch*: it prints `+` for a branch commit
     whose patch is NOT in `REF` and `-` for one already there. Integrated ⇔
     **no `+` line**:
     ```bash
     git cherry REF "$ARGUMENTS" | grep -q '^+' || echo "all patches already in REF"
     ```
     This is the case the old skill missed: a rebase-merged PR puts N *separate*
     patch-equivalent commits on `main`, so the single-commit squash test in (c)
     never matches even though every commit is upstream.
   - **(c) Squash-merge** — all commits collapsed into one new hash. Squash the
     branch to one commit and ask whether `REF` already contains that combined
     patch:
     ```bash
     MB=$(git merge-base "$ARGUMENTS" REF)
     SQUASH=$(git commit-tree "$ARGUMENTS^{tree}" -p "$MB" -m _)
     git cherry REF "$SQUASH" | grep -q '^-'   # a '-' line => combined patch already in REF
     ```

   When a PR number is known, `gh pr view <n> --json state,mergedAt,mergeCommit`
   is a fast confirmation — but the patch tests above are authoritative and work
   offline.

### Step 5: Delete Local Branch

- **Integrated** (safe — no work lost): delete with `git branch -D $ARGUMENTS`
  (use `-D`; `-d` recognizes neither squash- nor rebase-merges, and safety is
  already proven). Report which ref it was integrated into and by which path
  (merge / rebase / squash), e.g. "rebase-merged into `origin/main` via PR #53".
- **NOT integrated** (branch has unique, unmerged work): do **NOT** auto
  force-delete.
  1. List exactly what would be lost — branch commits whose **patch** is on
     *no* integration ref. Use `git cherry` (patch-equivalence), NOT
     `git log --not` (hash-reachability), or you will list rebased /
     cherry-picked commits that are already upstream. A commit is truly lost
     only if it is `+` against **every** ref that exists (intersection):
     ```bash
     # both refs exist: keep commits that are '+' on main AND on origin/main
     comm -12 \
       <(git cherry main        "$ARGUMENTS" | awk '/^\+/{print $2}' | sort) \
       <(git cherry origin/main "$ARGUMENTS" | awk '/^\+/{print $2}' | sort) \
       | xargs -r -n1 git show -s --oneline
     ```
     With only one ref available, just list that ref's `+` commits:
     `git cherry <ref> "$ARGUMENTS" | awk '/^\+/{print $2}' | xargs -r -n1 git show -s --oneline`.
  2. Ask the user to confirm force deletion.
  3. Only on explicit confirmation: `git branch -D $ARGUMENTS`.

### Step 6: Report Result

Report:
- Worktree path removed
- Branch deleted, and its integration status (which ref it was merged into, or
  that it was force-deleted with unmerged commits)
- Remaining worktrees: `git worktree list`
