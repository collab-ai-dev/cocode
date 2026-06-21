# coco-git

Git operations wrapper for commits, patches, ghost snapshots, and worktrees.

## Key Types

| Module | Exports |
|--------|---------|
| `operations` | `commit_all`, `ensure_git_repository`, `get_current_branch`, `get_head_commit`, `get_uncommitted_changes`, `is_inside_git_repo` |
| `apply` | `apply_git_patch`, `ApplyGitRequest` / `Result`, `stage_paths`, `extract_paths_from_patch`, `parse_git_apply_output` |
| `branch` | `merge_base_with_head` |
| `ghost_commits` | `create_ghost_commit` / `restore_ghost_commit` / `restore_to_commit`, `GhostSnapshotConfig` / `Report`, `IgnoredUntrackedFile`, `LargeUntrackedDir` |
| `worktree` | `cleanup_orphaned_worktrees`, `worktree_paths` (NFC-normalized list with 5s timeout), `parse_worktree_output` (pure parser exposed for tests) |
| `platform` | `create_symlink` |
| top-level | `GhostCommit` (TS-exportable via `ts_rs` + `JsonSchema`), `GitToolingError` |
