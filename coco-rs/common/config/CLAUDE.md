# coco-config

Layered config resolution: settings files, model/provider selection, effort/thinking, fast mode, env overrides, runtime overrides.

## Key Types

- `Settings`, `SettingsWithSource`, `SettingSource` (6 layers: Plugin < User < Project < Local < Flag < Policy)
- `SettingsWatcher` (debounced file watcher via `utils/file-watch`)
- `GlobalConfig` (~/.coco.json), `SessionSettings`
- `ModelInfo`, `ModelRoles`, `ModelAlias`, `RuntimeConfig`, `RuntimeOverrides`
- `EventHubConfig` (`event_hub_url` + `COCO_EVENT_HUB_URL` + CLI override; `ws://`/`wss://` only)
- `ServerConfig` — AppServer limits/timeouts/transport paths; each `server.*` key has a `COCO_SERVER_*` env override. See `sections.rs`.
- `ProviderConfig`, `ProviderInfo`
- `EnvOnlyConfig` (env-only overrides: just `model_override` + `force_env_auth` — Bedrock/Vertex/Foundry routing env vars were removed)
- `FastModeState` + `CooldownReason`
- `PlanModeSettings`, `PlanModeWorkflow`, `PlanPhase4Variant`
- `AnalyticsPipeline`, `AnalyticsSink`, `EventProperties`, `SessionAnalytics` (telemetry config surface)

## Scope

**Owned here**: `~/.coco.json`, `~/.coco/settings.json`, `.coco/settings.json`, `.coco/settings.local.json`, managed/enterprise settings, model capabilities cache, effort/fast-mode state.

**NOT owned**: CLAUDE.md (coco-context), .mcp.json (coco-mcp), skills/commands/hooks files (their respective crates). See `docs/internal/config-file-map.md`.

## Conventions

- `Settings.hooks` is `serde_json::Value` (deserialized by `coco-hooks`) — avoids L1→L4 dependency on feature crates.
- Per-setting source tracking via `SettingsWithSource` enforces security rules. `Project` (and `Plugin`) settings arrive with the checked-out repository, so they are **not** trusted for anything that can execute code or disarm the permission system. Enforced today via per-source accessors that exclude `Project` (all reading `TRUSTED_SETTING_SOURCES`): `api_key_helper()` (runs via `sh -c`), `startup_permission_mode()` and `disable_bypass_mode_enabled()` (bypass posture), `auto_mode_classify_all_shell_enabled()` and `use_auto_mode_during_plan_enabled()` (auto-mode), `enable_all_project_mcp_servers()` and `trusted_allowed_mcp_servers()` (repo-defined MCP server approval; resolved into `McpPolicyConfig` at the `build_runtime_config` merge site). `denied_mcp_servers()` is the deliberate inverse: unioned across **all** sources including `Project`, because a deny only narrows what can run. **A restriction only exists where such an accessor exists** — reading the same field off `Settings.merged` silently re-grants the project layer, so add the accessor before trusting a new high-risk field.
- Multi-provider API key resolution via `ProviderConfig.env_key` (each provider owns its env var). No Bedrock/Vertex/Foundry routing — those env vars were removed (see `env.rs`).
- Env vars are a **separate override layer**, not merged into `Settings`.
