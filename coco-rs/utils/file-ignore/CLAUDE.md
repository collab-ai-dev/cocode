# coco-file-ignore

Unified `.gitignore` / `.ignore` / `.agentignore` / custom-exclude filtering wrapped over the `ignore` crate.

## Key Types

| Type | Purpose |
|------|---------|
| `IgnoreService` | Main entry point; `new(config)` / `with_defaults()` → `create_walk_builder` |
| `IgnoreConfig` | Toggles: `respect_gitignore`, `respect_ignore`, `respect_agentignore`, `include_hidden`, `follow_links`, `custom_excludes`. Presets: `respecting_all` / `ignoring_none` / `for_glob_discovery` |
| `PatternMatcher`, `PathChecker` | Lower-level matcher primitives |
| `AGENT_IGNORE_FILE`, `IGNORE_FILE`, `IGNORE_FILES`, `find_ignore_files` | Ignore-file names + discovery helpers |
| `BINARY_FILE_PATTERNS`, `COMMON_IGNORE_PATTERNS`, `SYSTEM_FILE_EXCLUDES`, `COMMON_DIRECTORY_EXCLUDES`, `get_all_default_excludes` | Shared pattern constants |

Re-exports `ignore::WalkBuilder` so consumers don't need to depend on `ignore` directly.

## Ignore-file wiring (`create_walk_builder`)

Three matcher families, each driven by its own toggle, applied in **one** walk:

- `.gitignore` family (`git_ignore`/`git_global`/`git_exclude`) ← `respect_gitignore`
- `.ignore` (ripgrep native, `WalkBuilder::ignore(bool)`) ← `respect_ignore`
- `.agentignore` (via `add_custom_ignore_filename`) ← `respect_agentignore`

**`.agentignore` is honored independently of the other two toggles** — the
`ignore` crate builds custom-ignore matchers regardless of the git/ignore
toggles (`ignore-0.4.25/src/dir.rs`), so agent-hidden files stay hidden even in
the Glob tool's `--no-ignore` discovery mode (`for_glob_discovery`). That is the
"hide from the AI agent" guarantee for secrets/fixtures/generated artefacts.

Note: the Glob/Grep tools apply the model's glob *pattern* as a per-file
`Override` **matcher** (not the walker's whitelist override) precisely so it
cannot outrank `.agentignore` — a whitelist override beats every ignore file in
ripgrep. See `core/tools/src/tools/file_filter.rs`.

## Note

Direct edits to existing files are preferred over `*_ext.rs` extension modules for this crate.
