# coco-skills

Markdown workflow loading: bundled / user / project / plugin / managed / MCP sources, YAML frontmatter parsing, SKILL.md-directory discovery, dynamic per-path scanning.

## Key Types
- `SkillDefinition` — name, description, prompt, source, aliases, allowed_tools, model, when_to_use, argument_names, paths (globs), effort, `context` (Inline/Fork), agent, version, disabled, hooks, argument_hint, user_invocable, disable_model_invocation, shell, content_length, is_hidden
- `SkillContext` — `Inline` (expand into conversation) or `Fork` (sub-agent)
- `SkillSource` — `Bundled | User{path} | Project{path} | Plugin{plugin_name} | Managed{path} | Mcp{server_name}`
- `SkillManager` — name-keyed registry with alias lookup and `load_from_dirs`
- `SkillDirFormat` — `SkillMdOnly`; `Legacy` is parser-only compatibility and is not used by project/session discovery

## Key Functions
- `discover_skills()` / `discover_skills_with_format()` — walk dirs, dedup by canonical path, skip disabled
- `discover_skill_dirs_for_paths(file_paths, cwd)` — walk upward from each file to find nested `.coco/skills/` dirs
- `discover_dynamic_skills(dir)` — Read/Write/Edit hook for nested discovery
- `get_skill_paths(config_dir, project_dir)` — managed → user → project order
- `get_managed_skills_path()` — `/Library/Application Support/ClaudeCode/...` (macOS) or `/etc/claude-code/...`
- `load_skill_from_file()` / `parse_skill_markdown()` — `# Name` heading + YAML frontmatter
- `inject_skill_listing()` / `generate_skill_tool_prompt()` — 1% context-window budget, 250-char description cap, bundled skills never truncated
- `expand_braces()` — `*.{ts,tsx}` → `["*.ts","*.tsx"]` for `paths` globs
- `estimate_skill_tokens()` — frontmatter token estimation

## Modules
- `bundled` — compiled-in skill registry
- `shell_exec` — shell-backed skill execution
- `watcher` — skill-directory file watcher
