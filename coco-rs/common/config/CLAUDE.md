# coco-config

Layered config resolution: settings files, model/provider selection, effort/thinking, fast mode, env overrides, runtime overrides.

## Key Types

- `Settings`, `SettingsWithSource`, `SettingSource` (6 layers: Plugin < User < Project < Local < Flag < Policy)
- `SettingsWatcher` (debounced file watcher via `utils/file-watch`)
- `GlobalConfig` (~/.coco.json), `SessionSettings`
- `ModelInfo`, `ModelRoles`, `ModelAlias`, `RuntimeConfig`, `RuntimeOverrides`
- `EventHubConfig` (`event_hub_url` + `COCO_EVENT_HUB_URL` + CLI override; `ws://`/`wss://` only)
- `ServerConfig` (`server.unix_socket_path` + `COCO_SERVER_UNIX_SOCKET_PATH`, `server.websocket_bind` + `COCO_SERVER_WEBSOCKET_BIND`, `server.named_pipe_name` + `COCO_SERVER_NAMED_PIPE`, `server.max_sessions` + `COCO_SERVER_MAX_SESSIONS`, `server.max_surfaces_per_connection` + `COCO_SERVER_MAX_SURFACES_PER_CONNECTION`, `server.max_passive_surfaces_per_session` + `COCO_SERVER_MAX_PASSIVE_SURFACES_PER_SESSION`, `server.event_retention_per_session` + `COCO_SERVER_EVENT_RETENTION_PER_SESSION`, `server.outbound_queue_frames` + `COCO_SERVER_OUTBOUND_QUEUE_FRAMES`, `server.turn_drain_timeout_secs` + `COCO_SERVER_TURN_DRAIN_TIMEOUT_SECS`, `server.shutdown_timeout_secs` + `COCO_SERVER_SHUTDOWN_TIMEOUT_SECS`, `server.project_services_idle_ttl_secs` + `COCO_SERVER_PROJECT_SERVICES_IDLE_TTL_SECS` (default 3600), `server.idle_session_timeout_secs` + `COCO_SERVER_IDLE_SESSION_TIMEOUT_SECS` (default off); consumed by SDK AppServer sidecar listener startup, SDK AppServer max-session limits, local/SDK AppServer surface limits, queue/retention sizing, close-cascade turn drains, AppServer shutdown drains, `ProjectServices` idle eviction, and idle-session auto-archive)
- `ProviderConfig`, `ProviderInfo`
- `EnvOnlyConfig` (env-only overrides: Bedrock/Vertex/Foundry routing, model overrides, limits)
- `FastModeState` + `CooldownReason`
- `PlanModeSettings`, `PlanModeWorkflow`, `PlanPhase4Variant`
- `AnalyticsPipeline`, `AnalyticsSink`, `EventProperties`, `SessionAnalytics` (telemetry config surface)

## Scope

**Owned here**: `~/.coco.json`, `~/.coco/settings.json`, `.coco/settings.json`, `.coco/settings.local.json`, managed/enterprise settings, model capabilities cache, effort/fast-mode state.

**NOT owned**: CLAUDE.md (coco-context), .mcp.json (coco-mcp), skills/commands/hooks files (their respective crates). See `docs/internal/config-file-map.md`.

## Conventions

- `Settings.hooks` is `serde_json::Value` (deserialized by `coco-hooks`) — avoids L1→L4 dependency on feature crates.
- Per-setting source tracking via `SettingsWithSource` enforces security rules. `Project` (and `Plugin`) settings arrive with the checked-out repository, so they are **not** trusted for anything that can execute code or disarm the permission system. Enforced today via per-source accessors that exclude `Project` (all reading `TRUSTED_SETTING_SOURCES`): `api_key_helper()` (runs via `sh -c`), `startup_permission_mode()` and `disable_bypass_mode_enabled()` (bypass posture), `auto_mode_classify_all_shell_enabled()` and `use_auto_mode_during_plan_enabled()` (auto-mode). **A restriction only exists where such an accessor exists** — reading the same field off `Settings.merged` silently re-grants the project layer, so add the accessor before trusting a new high-risk field.
- Multi-provider API key resolution via `ProviderConfig.env_key` (each provider owns its env var); `EnvOnlyConfig` handles Anthropic-specific Bedrock/Vertex/Foundry routing only.
- Env vars are a **separate override layer**, not merged into `Settings`.
