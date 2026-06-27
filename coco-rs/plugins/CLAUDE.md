# coco-plugins

Plugin system: PLUGIN.toml manifests, bundled/user/project/repository sources, contribution discovery (skills / hooks / MCP servers / agents / commands), enable/disable, marketplace, hot-reload, MCPB (MCP Bundle) loading.

## Key Types

The loader is **single-tier V2** — no legacy
name-keyed `PluginManager`; the active set is computed fresh each load and the
contribution bridges register from it.

- `PluginManifestV2` (`schemas`) — name, version, description, dependencies,
  and contribution declarations (skills / hooks / agents / commands / mcp_servers
  / lsp_servers / output_styles). Deserializes from PLUGIN.toml **or** plugin.json.
- `PluginId` (`schemas`) — `name` + `marketplace`; Display = `name@marketplace`.
  Reserved synthetic marketplaces: `inline` (local standing-dir plugins) and
  `builtin`. (`identifier::PluginId` is the settings-layer twin used by the
  install/enable handlers; both render identically.)
- `LoadedPluginV2` (`loader`) — `id`, `manifest`, `path`, `load_source`
  (`Marketplace{marketplace} | SessionDir | Builtin`), `enabled`.
- `PluginLoader` (`loader`) — `load_from_dir` (validates one dir),
  `load_from_marketplace`, and `load_all_plugins` (the orchestrator: marketplace
  cache + inline dirs, inline-overrides-marketplace-by-name, dependency-closure
  demotion via `verify_and_demote`).
- `load_enabled_plugins(config_home, project_dir)` — production entry point:
  resolves marketplaces + standing dirs, gates on settings.json
  `enabled_plugins`, returns the enabled `Vec<LoadedPluginV2>`. The session
  bootstrap and `/reload-plugins` register commands / hooks / skills from this.
- `get_plugin_dirs(config_dir, project_dir)` — `~/.coco/plugins/*/` +
  `.claude/plugins/*/` (the inline standing dirs).

## Modules
- `loader` — manifest reading, per-dir validation, and the `load_all_plugins` orchestrator
- `marketplace` — manifest fetch/reconcile/dependency resolution
- `hot_reload` / `watcher` — change detection (surfaces *that* something changed; refresh is the explicit `/reload-plugins` action)
- `schemas` — manifest + marketplace + `PluginId` schemas
- `command_bridge` / `hook_bridge` / `skill_bridge` — wire `LoadedPluginV2` contributions into `CommandRegistry` / `HookRegistry` / `SkillManager`
