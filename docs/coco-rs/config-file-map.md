# Config File Ownership Map (Source of Truth)

Every file coco-rs reads from or writes to, which crate owns it.

**Principle**: Each crate owns its own config files. `coco-config` only owns settings/model config, NOT all `.claude/` files.

---

## Project-Level Files (`.claude/` + project root)

| Path | Format | Owner Crate | R/W | What it stores |
|------|--------|-------------|-----|----------------|
| `.claude/settings.json` | JSON | `coco-config` | RW | Project shared settings (checked in) |
| `.claude/settings.local.json` | JSON | `coco-config` | RW | Project local settings (gitignored) |
| `.mcp.json` | JSON | `coco-mcp` | RW | Project-scoped MCP server configuration |
| `CLAUDE.md` | Markdown | `coco-context` | R | Project instructions (root level) |
| `CLAUDE.local.md` | Markdown | `coco-context` | R | Local project instructions (gitignored) |
| `.claude/CLAUDE.md` | Markdown | `coco-context` | R | Project instructions (alt location) |
| `.claude/rules/*.md` | Markdown+YAML | `coco-context` | R | Project rules (code-style, testing, security, etc.) |
| `.claude/skills/*.md` | Markdown+YAML | `coco-skills` | R | Project-scoped skills/slash commands |
| `.claude/commands/*.md` | Markdown+YAML | `coco-commands` | R | Legacy project commands (superseded by skills/) |
| `.claude/agents/*.md` | Markdown+YAML | `coco-query` | R | Project-scoped agent definitions |
| `.claude/output-styles/*.md` | Markdown | `coco-tui` | R | Project output formatting styles |
| `.claude/workflows/` | Markdown | `coco-skills` | R | Project workflow definitions |
| `.coco/scheduled_tasks.json` | JSON | `coco-tool-runtime` | RW | Persistent (durable) cron tasks |
| `.coco/scheduled_tasks.lock` | Lock | `coco-tool-runtime` | RW | (deferred) cross-process lease lock for cron |
| `.claude/agent-memory/<type>/` | Text | `memory/` | RW | Project-scope agent persistent memory |
| `.claude/agent-memory-local/<type>/` | Text | `memory/` | RW | Local-scope agent memory (not shared) |
| `.claude/agent-memory-snapshots/<type>/` | Text | `memory/` | RW | Agent memory snapshots |
| `.claude/worktrees/` | Git | `coco-tools` | RW | Git worktrees (EnterWorktreeTool) |

---

## Global Files (`~/.coco/` + home)

| Path | Format | Owner Crate | R/W | What it stores |
|------|--------|-------------|-----|----------------|
| `~/.coco.json` | JSON | `coco-config` | RW | GlobalConfig: user ID, theme, project configs, costs |
| `~/.coco/settings.json` | JSON | `coco-config` | RW | User global settings |
| `~/.coco/keybindings.json` | JSON | `keybindings/` | RW | Keyboard shortcuts |
| `~/.coco/theme.json` | JSON | `coco-tui` | RW | TUI active theme and custom color palettes |
| `~/.coco/CLAUDE.md` | Markdown | `coco-context` | R | User global instructions |
| `~/.coco/rules/*.md` | Markdown+YAML | `coco-context` | R | User global rules |
| `~/.coco/skills/*.md` | Markdown+YAML | `coco-skills` | R | User personal skills |
| `~/.coco/agents/*.md` | Markdown+YAML | `coco-query` | R | User personal agent definitions |
| `~/.coco/commands/*.md` | Markdown+YAML | `coco-commands` | R | Legacy user commands |
| `~/.coco/output-styles/*.md` | Markdown | `coco-tui` | R | User output styles |
| `~/.coco/plans/*.md` | Markdown | `coco-query` | RW | Plan files |
| `~/.coco/projects/<hash>/memory/` | Markdown | `memory/` | RW | Auto-memory per project |
| `~/.coco/projects/<hash>/memory/MEMORY.md` | Markdown | `memory/` | RW | Memory index entrypoint |
| `~/.coco/projects/<hash>/memory/logs/` | Markdown | `memory/` | RW | Daily memory logs |
| `~/.coco/projects/<hash>/*.jsonl` | JSONL | `coco-session` | RW | Session transcripts |
| `~/.coco/history.jsonl` | JSONL | `coco-messages` | RW | Global REPL history |
| `~/.coco/debug/*.txt` | Text | `coco-otel` | W | Debug logs |
| `~/.coco/cache/` | Various | `coco-config` | RW | Cached model capabilities, changelog |
| `~/.coco/plugins/cache/` | Various | `plugins/` | RW | Plugin installations |
| `~/.coco/teams/<name>/` | JSON | `coco-tasks` | RW | Team configs (v2) |
| `~/.coco/server-sessions.json` | JSON | `coco-cli` | RW | Server mode session registry |
| `~/.coco/agent-memory/<type>/` | Text | `memory/` | RW | User-scope agent memory |

---

## Enterprise/Managed Files

| Path | Format | Owner Crate | R/W | What it stores |
|------|--------|-------------|-----|----------------|
| `/etc/coco/managed-settings.json` (Linux) | JSON | `coco-config` | R | Enterprise policy base |
| `/etc/coco/managed-settings.d/*.json` | JSON | `coco-config` | R | Enterprise policy fragments |
| `/etc/coco/managed-mcp.json` | JSON | `coco-mcp` | R | Enterprise MCP server config |
| `/Library/Application Support/CoCo/` (macOS) | Various | `coco-config` | R | macOS managed settings |
| MDM plist / Windows HKLM registry | Various | `coco-config` | R | OS-level policy distribution |

---

## Crate Ownership Summary

| Crate | Files it owns | Count |
|-------|--------------|-------|
| `coco-config` | settings.json (user, project, local, managed), GlobalConfig, cache/ | 8 |
| `coco-context` | CLAUDE.md (root, .claude/, user), CLAUDE.local.md, rules/*.md | 6 |
| `coco-mcp` | .mcp.json, managed-mcp.json | 2 |
| `coco-skills` | .claude/skills/*.md, ~/.coco/skills/*.md, workflows/ | 3 |
| `coco-commands` | .claude/commands/*.md, ~/.coco/commands/*.md | 2 |
| `coco-query` | agents/*.md, plans/*.md | 4 |
| `coco-tasks` | scheduled_tasks.json, teams/ | 3 |
| `memory/` | agent-memory*/, projects/*/memory/ | 5 |
| `coco-messages` | history.jsonl | 1 |
| `coco-session` | projects/*/*.jsonl, server-sessions.json | 2 |
| `coco-tui` | output-styles/*.md | 2 |
| `keybindings/` | keybindings.json | 1 |
| `plugins/` | plugins/cache/ | 1 |
| `coco-otel` | debug/*.txt | 1 |
| `coco-tools` | worktrees/ | 1 |
| `coco-cli` | server-sessions.json | 1 |

---

## Key Design Rules

1. **coco-config does NOT own all `.claude/` files** — each crate owns its own config
2. **File discovery uses `utils/common::find_coco_home()`** — never hardcode `~/.claude`
3. **File paths use `utils/absolute-path::AbsolutePathBuf`** — consistent normalization
4. **File watching uses `utils/file-watch::FileWatcher`** — only for settings + CLAUDE.md (not all files)
5. **JSON files use `serde_json`; Markdown+YAML uses frontmatter parser** — consistent across crates
6. **Write operations use file locking** — prevents corruption from concurrent processes
7. **Project files at `.claude/` may be checked in** — `settings.local.json`, `CLAUDE.local.md`, `agent-memory-local/` are gitignored
