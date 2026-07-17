# Memory

cocode reads instruction files from your project and your home directory and folds them into the system prompt at the start of every session. This page covers how those files are discovered, how `@import` works, how conditional rules are loaded, and the status of the auto-memory subsystem.

## Memory files

The files cocode looks for are:

| File | Meaning |
|------|---------|
| `CLAUDE.md` | Project instructions. Commit this. |
| `AGENTS.md` | Equivalent to `CLAUDE.md`. Supported so a repo that already uses the cross-tool convention works unchanged. |
| `CLAUDE.local.md` | Personal instructions for this directory. Do not commit; add it to `.gitignore`. |
| `AGENTS.local.md` | Equivalent to `CLAUDE.local.md`. |

**Name matching is case-insensitive at every position.** `claude.md`, `Claude.MD`, and `AGENTS.md` all match. The one exception is `<dir>/.cocode/CLAUDE.md`, which is probed as a literal filename — `.cocode/agents.md` and `.cocode/claude.md` are not picked up there.

**Byte-identical duplicates in one directory collapse to a single file.** If a directory contains both `CLAUDE.md` and `AGENTS.md` and their contents are byte-for-byte identical, only `CLAUDE.md` is loaded — so the common trick of symlinking or copying one to the other does not double your token cost. If the contents differ at all, both files load. Only files of equal size are ever compared, so this check is cheap.

## Discovery order

Everything below is discovered eagerly at session start and rendered into the system prompt in this order. Later files do not override earlier ones — they all load and stack up, so treat the order as "most general first, most specific last".

1. **Managed** — org-pushed policy memory, if present.
2. **User-global** — `~/.cocode/CLAUDE.md` (or `AGENTS.md`), then `~/.cocode/rules/*.md`. These apply to every project you work on.
3. **The walk from the filesystem root down to your current directory.** For each directory along the way, in this order:
   1. `<dir>/.cocode/CLAUDE.md`
   2. `<dir>/CLAUDE.md` or `<dir>/AGENTS.md`
   3. `<dir>/.cocode/rules/*.md` — unconditional rules only, see below
   4. `<dir>/CLAUDE.local.md` or `<dir>/AGENTS.local.md`

Because the walk starts at the filesystem root, a `CLAUDE.md` in a parent directory of your repo also applies. In a monorepo this is what lets a package-level `CLAUDE.md` stack on top of the repository-root one.

### Size limits

Each memory file is capped at **4,000 bytes** and the eager set as a whole is capped at **24,000 bytes** when rendered into the system prompt. Content past either cap is cut at a UTF-8 boundary and marked with `[Memory file truncated]`. Keep individual files well under 4 KB; push detail into `@import`ed files or conditional rules rather than one enormous `CLAUDE.md`.

Note that these caps apply to interactive sessions. Headless (`-p`) runs and subagent prompts assemble their system prompt through a different path that does not truncate, so a large memory file behaves differently across the two.

## `@import`

A memory file can pull in another file with `@import`:

```markdown
# Project instructions

Standard build commands are in @import ./docs/build.md

@import ~/.cocode/my-personal-style.md
```

Imports are expanded recursively to a **maximum depth of 5**. The file at depth 5 still loads; its own imports are simply not followed. Cycles are broken by canonicalizing each path and refusing to process one twice, so `a.md` importing `b.md` importing `a.md` is safe and terminates.

**Only user-global memory may import files from outside the project.** A `CLAUDE.md` in your repo can only import paths contained within the current working directory — an import pointing elsewhere is silently skipped. This containment check applies to project, project-config, local, *and* managed memory. Files under `~/.cocode/` are the sole exception and may import from anywhere, which is what makes the second example above work.

## Rules

`.cocode/rules/*.md` holds instructions that are either always active or scoped to particular files. Any `*.md` file under the rules directory qualifies — unlike memory files, rules are identified by content, not filename, so the `CLAUDE.md`/`AGENTS.md` naming does not apply here.

A rule with no frontmatter, or no `paths:` key, is **unconditional**: it loads eagerly with the rest of your memory.

A rule with a `paths:` key in its YAML frontmatter is **conditional**: it stays out of the system prompt and is only injected when the model reads a file matching one of its patterns.

```markdown
---
paths:
  - "src/**/*.rs"
  - "build.rs"
---

Rust in this repo targets stable. Never add a nightly-only feature gate.
```

`paths:` accepts a YAML sequence, as above, or a comma-separated string (`paths: "src/**/*.rs, build.rs"`). Brace alternation like `{a,b}` is expanded, and a trailing `/**` is stripped.

Patterns are matched with **gitignore semantics**, not plain globbing — the same rules your `.gitignore` follows. Project rule patterns are matched relative to the project directory; managed and user-global rule patterns are matched relative to your original working directory.

Conditional rules are the right place for anything long and narrow: a 300-line style guide for one subsystem costs nothing until the model opens a file in it.

## Nested worktrees

When your working directory is a git worktree nested inside its own main repository, the root-to-cwd walk passes through both the main repo root and the worktree root. Git has already checked the branch's tracked memory out into the worktree, so the same content exists at two paths and would otherwise load twice.

To prevent that, cocode skips **checked-in** memory — `.cocode/CLAUDE.md`, `CLAUDE.md`/`AGENTS.md`, and unconditional `.cocode/rules/*.md` — in directories that sit inside the main repo but above the worktree root. `CLAUDE.local.md` and `AGENTS.local.md` are never skipped, because they are gitignored and therefore exist only in the main checkout, not duplicated into the worktree.

## Commands

`/memory` lists the memory files it finds with their paths, line counts, and sizes. `/memory refresh` reports that files reload on the next turn — edits to a `CLAUDE.md` take effect without restarting the session.

`/init` bootstraps a `CLAUDE.md` for the current repository (and optionally skills and hooks) by having the model explore the codebase and write the file. Run it once in a new project.

To add a memory file by hand, just create it. There is no registration step.

## Auto-memory

> **Status: off by default, and under development.** Auto-memory is gated by the `auto_memory` feature, which is marked `UnderDevelopment` and defaults to **false**. It is not a finished feature. The description below is accurate about what the code does when you switch it on, but treat it as unstable rather than something to build a workflow around.

Everything above — `CLAUDE.md` discovery, imports, rules — works regardless of this gate. Auto-memory is a separate, additive subsystem that lets the agent write its own memory rather than only reading yours.

Turn it on with:

```jsonc
// ~/.cocode/settings.json
{
  "features": {
    "auto_memory": true
  }
}
```

When enabled, three things become available:

**Extraction.** At the end of a turn, a forked agent reviews what happened and writes durable notes into a per-project memory directory. It is throttled and turn-bounded so it does not run on every exchange. Subagents never extract — only the main thread does.

**Session memory.** A running summary of the current session, maintained incrementally as tokens and tool calls accumulate, with per-section and total token budgets.

**Auto-dream (consolidation).** A periodic pass that consolidates accumulated memory. It is guarded by three gates before it will spend tokens: at least 24 hours since the last run, at least 5 distinct sessions of material, and a process lock so two cocode instances cannot dream at once.

Sub-toggles live under the `memory` key in settings and only matter once `auto_memory` is on:

```jsonc
// ~/.cocode/settings.json
{
  "features": {
    "auto_memory": true
  },
  "memory": {
    "extraction_enabled": true, // turn-end extraction fork
    "extraction_throttle": 1,
    "extraction_max_turns": 5,
    "dream_enabled": true, // periodic consolidation
    "dream_min_hours": 24,
    "dream_min_sessions": 5,
    "session_memory_enabled": true,
    "team_memory_enabled": false, // shared team memory subdirectory
    "directory": null // override the memory directory outright
  }
}
```

**`/dream` and `/summary` only exist when `auto_memory` is on.** They are registered conditionally, so on a default install they are not merely inert — the commands are absent from the registry entirely and `/dream` will report an unknown command. If you enabled the feature and still do not see them, the subsystem also deactivates under bare mode and in remote sessions that have no memory directory configured.

See [configuration](configuration.md) for how feature flags and settings layer.
