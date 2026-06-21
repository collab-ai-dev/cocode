# coco-cli

Top-level CLI: clap parser, binary entry, SDK (NDJSON over stdio), subcommand dispatch.
Depends on everything — wires registries, builds model runtime registry, starts TUI or SDK server.

## Key Types

| Type | Purpose |
|------|---------|
| `Cli` (clap `Parser`) | Binary name `coco`; see `lib.rs` for flags |
| `Commands` | Subcommands: Chat, Config, Resume, Sessions, Status, Doctor, Login, Mcp, Plugin, Daemon, Ps/Logs/Attach/Kill, RemoteControl, Sdk, ReleaseNotes, Upgrade, Agents, AutoMode |
| `{Config,Mcp,Plugin}Action` | Subcommand action enums |
| `sdk_server::SdkServer` | NDJSON control server (Commands::Sdk) |
| `sdk_server::StdioTransport` | stdin/stdout NDJSON transport |
| `sdk_server::QueryEngineRunner` | Bridges `QueryEngine` to SDK control messages |
| `sdk_server::CliInitializeBootstrap` | Session bootstrap from `initialize` control request |
| `sdk::ModelUsage` + schemas | SDK wire types (mirrors `coreSchemas.ts`) |
| `tui_runner::*` | Launches `coco-tui` after bootstrap |
| `model_factory::*` | Builds `Arc<dyn LanguageModelV4>` from provider/model config |
| `output::*` | Non-interactive output formatters (text/json/stream-json) |

## Startup Flow

1. `Cli::parse()` — clap parses argv (subcommand or default chat)
2. Fast-path subcommands (Config, Doctor, ReleaseNotes, Sessions, Upgrade) bypass QueryEngine
3. Interactive/print/SDK paths: load config, build `ModelRuntimeRegistry`, register tools + commands
4. `Sdk` → `sdk_server` (NDJSON over stdio, `initialize`/`interrupt`/`can_use_tool`/`set_permission_mode`/...)
5. `--non-interactive` (print mode) → single `QueryEngine::run` + `output::*` formatter
6. Interactive → `tui_runner::run` (launches `coco-tui`)

## Flag Highlights

Session: `--prompt`, `--output-format`, `--input-format`, `--json-schema`, `--max-turns`, `--max-budget-usd`
Resume: `--continue`, `--resume`, `--fork-session`, `--session-id`, `--name`
Auth/Perms: `--dangerously-skip-permissions`, `--allow-dangerously-skip-permissions`, `--permission-mode`, `--permission-prompt-tool`
Tools: `--allowed-tools`, `--disallowed-tools`, `--add-dir`
Config: `--settings`, `--setting-sources`, `--system-prompt`, `--append-system-prompt(-file)`, `--mcp-config`, `--strict-mcp-config`
Model: `--models.main`, `--fallback-model`, `--betas`, `--agent`, `--thinking`, `--thinking-budget`, `--max-thinking-tokens`, `--effort`
Worktree/bg: `--worktree`, `--bg`
SDK: `--replay-user-messages`, `--include-hook-events`, `--include-partial-messages`

## Stop Hooks Dispatch Order

Post-turn hooks fire from `coco_query::engine_finalize_turn` in this order:

1. **bareMode gate** — `--bare` mode skips all post-turn forks (no
   prompt suggestion, no memory extraction, no auto-dream). Used
   by SDK / scripted `-p` invocations that don't want background
   work after each turn.
2. **promptSuggestion** — fires unconditionally (subject to its
   own 9-step guard sequence in
   `coco_query::prompt_suggestion::try_generate_suggestion`).
3. **extractMemories** — fires when `MemoryConfig.extraction_enabled`
   AND `agent_id.is_none()` (subagents don't extract).
4. **autoDream** — fires when `MemoryConfig.dream_enabled` AND
   `agent_id.is_none()`. The 3-gate scheduler (`memory/src/service/dream.rs`)
   then internally checks 24h elapsed + 5 distinct sessions + PID
   lock before paying for the consolidation.

Each fork dispatches via `coco_query::forked_agent::ForkDispatcher`
(installed by `fork_dispatcher::install` at session bootstrap).
The dispatcher threads the parent's `CacheSafeParams` so the child's API
request prefix matches byte-for-byte. Per-fork `canUseTool` policies live in
`coco-memory::can_use_tool` (auto-mem + session-mem); promptSuggestion
+ side_question + agent_summary use `deny_all_handle`.

## Allocator (jemalloc)

jemalloc is the global allocator for release/distribution builds, installed via
`#[global_allocator]` in `src/main.rs` — opt-in behind the `jemalloc` Cargo
feature, never on Windows (no jemalloc-sys MSVC build). Tuning
(`dirty_decay_ms` / `muzzy_decay_ms` / `narenas`) is **baked into libjemalloc at
build time** via `JEMALLOC_SYS_WITH_MALLOC_CONF` in `.cargo/config.toml`
(`--with-malloc-conf`), which is why it applies on **both Linux and macOS** —
the exported `malloc_conf` symbol form would be ignored on macOS's `_rjem_`-
prefixed build. Per-knob meanings and defaults are documented inline in `.cargo/config.toml`.
Three ways to set the tuning, by when it binds:

- **Build-time baseline** (current). Edit the `JEMALLOC_SYS_WITH_MALLOC_CONF`
  string and rebuild. Setting that env at launch does nothing — it is consumed
  only by the jemalloc-sys build script.
- **Startup override, no rebuild.** jemalloc reads its own env at init (a later
  conf source, so it overrides the baked baseline — including `narenas`): set
  `MALLOC_CONF` on Linux, `_RJEM_MALLOC_CONF` on macOS (the `_rjem_`-prefixed
  build). Caveat: init runs before `main` (on the first allocation), so it must
  be present in the environment **before exec** — `coco` cannot set it for
  itself, and settings.json (parsed post-init) cannot drive it.
- **Live, in-process.** Only the decay knobs (`arena.<i>.dirty_decay_ms` /
  `muzzy_decay_ms`, both `rw` mallctl) are mutable after init, and only via a
  `tikv-jemalloc-ctl` dep that is **not currently wired**. `narenas` is never
  runtime-mutable post-init.
