# coco-config

Layered config resolution: settings files, model/provider selection, effort/thinking, fast mode, env overrides, runtime overrides.

## Key Types

- `Settings`, `SettingsWithSource`, `SettingSource` (6 layers: Plugin < User < Project < Local < Flag < Policy)
- `SettingsWatcher` (debounced file watcher via `utils/file-watch`)
- `GlobalConfig` (~/.coco.json), `SessionSettings`
- `ModelInfo`, `ModelRoles`, `ModelAlias`, `RuntimeConfig`, `RuntimeOverrides`
- `ProviderConfig`, `ProviderInfo`
- `EnvOnlyConfig` (env-only overrides: Bedrock/Vertex/Foundry routing, model overrides, limits)
- `FastModeState` + `CooldownReason`
- `PlanModeSettings`, `PlanModeWorkflow`, `PlanPhase4Variant`
- `AnalyticsPipeline`, `AnalyticsSink`, `EventProperties`, `SessionAnalytics` (telemetry config surface)

## Scope

**Owned here**: `~/.coco.json`, `~/.coco/settings.json`, `.coco/settings.json`, `.coco/settings.local.json`, managed/enterprise settings, model capabilities cache, effort/fast-mode state.

**NOT owned**: CLAUDE.md (coco-context), .mcp.json (coco-mcp), skills/commands/hooks files (their respective crates). See `docs/coco-rs/config-file-map.md`.

## Conventions

- `Settings.hooks` is `serde_json::Value` (deserialized by `coco-hooks`) — avoids L1→L4 dependency on feature crates.
- Per-setting source tracking via `SettingsWithSource` enforces security rules (e.g. project settings cannot set `api_key_helper`, auto-mode config, bypass mode).
- Multi-provider API key resolution via `ProviderConfig.env_key` (each provider owns its env var); `EnvOnlyConfig` handles Anthropic-specific Bedrock/Vertex/Foundry routing only.
- Env vars are a **separate override layer**, not merged into `Settings`.
