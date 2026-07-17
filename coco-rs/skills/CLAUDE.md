# coco-skills

Markdown workflow loading: bundled / user / project / plugin / managed / MCP / agent-created sources, YAML frontmatter parsing, SKILL.md-directory discovery, dynamic per-path scanning, lazy prompt rendering.

## Key Types
- `SkillDefinition` — frontmatter-driven skill record (name, description, prompt, source, aliases, allowed_tools, model, `context`, paths globs, hooks, shell, disable_model_invocation, provenance, …)
- `SkillContext` — `Inline` (expand into conversation) or `Fork` (sub-agent)
- `SkillSource` — `Bundled | User{path} | Project{path} | Plugin{plugin_name} | Managed{path} | Mcp{server_name}`. This is the *scope* axis.
- `SkillOrigin` — *authorship* axis, distinct from source. `User` is the fail-safe `Default`; only `Agent` enters Curator/skill-learn flows.
- `SkillManager` — name-keyed registry with alias lookup
- `SkillLoadGates` / `SkillScopes` — per-scope gates for `build_session_skill_manager` (managed / agent / user / project / legacy commands / `--add-dir`); `skills_locked` (`strictPluginOnlyCustomization`) loads managed only
- `SkillDirFormat` — `SkillMdOnly`; `Legacy` (flat `.md`) is parser-only compatibility, not used by project/session discovery

## Module Map
- `lib.rs` — discovery (`discover_skills*`, `discover_skill_dirs_for_paths`, `discover_dynamic_skills`), parsing (`load_skill_from_file` → private `parse_skill_markdown`), session composition (`build_session_skill_manager`, `get_skill_paths`), listing injection (`inject_skill_listing`, `generate_skill_tool_prompt`), `expand_braces`, `estimate_skill_tokens`
- `agent_scope` — agent-created skills scope (see below)
- `bundled` — compiled-in skill registry
- `extraction` — bundled-skill file extraction: per-process nonce dir under `<config_home>/bundled-skills/`, dirs 0o700 / files 0o600, `O_EXCL|O_NOFOLLOW` writes, path validation rejects absolute + `..` segments, memoized one extraction per skill; failure degrades to prompt-without-base-dir
- `mcp_builders` — write-once builder registry for MCP-sourced skills; breaks the `coco-mcp` (L3) → `coco-skills` (L4) layer cycle — app layer registers the builder at startup, second registration is a no-op
- `overrides` — pure `skill_overrides` resolution: `resolve_skill_override_lock` (dialog lock display), `resolve_skill_baseline` (diff-against-baseline save), `effective_skill_state` (Skill-tool gate + listing filter; plugin source short-circuits `On`). `disable_model_invocation` is deliberately not folded into effective state.
- `prompt_render` — lazy rendering: `$1`/`$ARGUMENTS` substitution, embedded shell (```` ```! ```` blocks + `` !`…` `` spans via `shell_exec`), `Base directory for this skill:` prefix after extraction; `PromptPart` maps 1:1 to vercel-ai Text/File parts
- `reminder_source` — `SkillsSource` impl for the reminder pipeline: per-entry 250-char description cap, never drops a skill (the shrink-to-names budget path stays in `generate_skill_tool_prompt`)
- `shell_exec` — shell-backed skill execution
- `telemetry` — per-skill lifecycle counts (success/failure/view/patch) feeding the skill-learn Curator; no debounce, in-process file lock, atomic-rename writes; deliberately separate from `usage` so failures can't pollute autocomplete ranking
- `usage` — `/` autocomplete recency ranker: 7-day half-life, min recency factor 0.1, 60 s debounce, atomic-rename writes to `<config_home>/skill_usage.json`
- `watcher` — skill-directory file watcher

## Agent-Created Scope (`agent_scope`) — load-bearing seam
- Location-keyed: everything under `<config_home>/skills/.agent/<name>/SKILL.md` is agent-authored **by location**; the LLM-written frontmatter is untrusted, so enforcement overrides whatever the file claims.
- Loads inert: `provenance.origin` force-stamped `SkillOrigin::Agent`; `allowed_tools` / `hooks` / `shell` force-dropped — an agent skill can never self-fire a shell, install hooks, or widen permissions.
- `disable_model_invocation` is owned by the Curator's promotion state, not the file: quarantined until promoted. Promotions live OUTSIDE the fenced root (`promotions_path`) so a prompt-injected review fork cannot self-promote what it writes. Users can still invoke quarantined skills via `/name` — that accrues the telemetry the Curator promotes/retires on.
- Parse-time neutralization in `parse_skill_markdown` (frontmatter `origin: agent`) remains as defense-in-depth for files copied OUT of the directory.
- Gated by `Feature::SkillLearning` at the call site (`SkillLoadGates::agent_skills_enabled`) and additionally requires `user_enabled`.
- `coco-skill-learn` depends on this module (runtime, review, curator, journal).

## Invariants
- Listing budget: `generate_skill_tool_prompt` caps at 1% of the context window (tokens × 0.01 × 4 chars, min 2000), 250-char per-description cap, bundled skills never truncated out.
- `get_managed_skills_path()` — managed base + `.cocode`: `/Library/Application Support/ClaudeCode/.cocode/skills` (macOS), `/etc/claude-code/.cocode/skills` (else). `get_managed_commands_path()` same base + `commands`.
- `get_skill_paths(config_dir, project_dir)` — managed → user → project walk-up order.
- `expand_braces()` — `*.{ts,tsx}` → `["*.ts","*.tsx"]` for `paths` globs.
