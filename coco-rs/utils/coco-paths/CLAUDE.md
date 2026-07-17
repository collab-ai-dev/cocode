# coco-paths

Single source of truth for the `projects/<slug>/` path layout (projects, sessions,
memory). Exists because divergent copies of `sanitizePath` once produced slugs that
did NOT match Claude Code's — silent cross-tool data isolation, no error surfaced.

## Key API

| Item | Purpose |
|------|---------|
| `ProjectSlug::for_path` | THE canonical slug pipeline: `normalize_nfc` → `sanitize_path` on the project root |
| `sanitize_path` | JS-equivalent sanitizer: UTF-16 code units → `[a-zA-Z0-9]` or `-`; >200 chars appends `-{djb2 base36}` |
| `simple_hash` / `djb2` | JS `Math.abs(djb2Hash(s)).toString(36)` — i32 wrap-on-overflow, UTF-16 units |
| `ProjectPaths` | Per-project path facade: transcripts, subagent/session dirs, tool-results, memory, daily logs, session locks |
| `RuntimePaths` | Config-home vs memory-base split (sessions, logs, plugins, output-styles, models.json) |
| `project_dir` / `find_project_dir` | Pure slug join; on-disk lookup with long-path prefix fallback for Bun-vs-Node hash divergence |
| `normalize_lexical` / `relative_posix_path` | Symlink-free lexical normalization for permission/config checks |

## Invariants

- **Never reimplement slug logic.** Every `projects/<slug>/` name must match Claude Code's layout — always go through `ProjectSlug` / `sanitize_path`, so a TS run and a coco-rs run on the same cwd land in the same directory.
- Slug input should be the canonical git root (linked worktrees share one slug), falling back to cwd; git resolution is the caller's job via `coco_git` — this crate stays dependency-light (no env reads, no subprocesses).
- Long slugs (>200) match TS-on-Node (djb2) and may diverge from TS-on-Bun (`Bun.hash`); `find_project_dir`'s prefix scan absorbs that documented trade-off.
