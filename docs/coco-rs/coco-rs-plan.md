# coco-rs: TS-First Migration Plan

## Context

Create `coco-rs` — a Rust code agent whose **core logic follows the Claude Code TypeScript implementation**, while leveraging cocode-rs's base infrastructure (error handling, utils, vercel-ai multi-provider SDK) and Rust best practices.

**Principles**:
1. **TS-first**: Each TS source module maps to a Rust crate. TS defines the architecture.
2. **Rust best practices for details**: snafu errors, trait-based tool system, CancellationToken, Arc sharing
3. **Copy only base infra from cocode-rs**: error, otel, utils, vercel-ai. Everything else is redesigned.
4. **provider-sdks removed**: vercel-ai handles all provider abstraction directly
5. **ModelInfo/config redesigned**: follows TS's model selection system (aliases, effort, fast mode, capabilities)

### Review Findings

**Dependency fixes**:
- `exec/sandbox` depends on `cocode_protocol::SandboxMode` — must define `SandboxMode` in `coco-types`
- `exec/shell` declares unused `cocode-protocol` dep — remove from Cargo.toml
- All 24 utils, 8 vercel-ai, lsp, process-hardening: verified clean of removed crates

**Missing TS modules added**:
- `utils/attachments.ts` (4K LOC) -> `coco-context`
- `services/analytics/` (4K LOC) -> `coco-otel` (原映射 coco-inference 已修正)
- `services/tokenEstimation.ts`, `services/oauth/`, rate limiting -> `coco-inference`
- `services/SessionMemory/` + `services/autoDream/` -> `coco-memory`
- `services/remoteManagedSettings/` + `services/settingsSync/` + `migrations/` -> `coco-config`
- `utils/bash/` (12K LOC) + `utils/Shell.ts` + `utils/shell/` -> `coco-shell`
- `services/notifier.ts` -> `coco-tui`, `cli/` transports -> `coco-cli`

### Justification for Every cocode-rs Choice

When the plan keeps cocode-rs code instead of rewriting from TS, one of these reasons **must** apply:

| Reason | Meaning |
|--------|---------|
| **Rust-only** | Uses libc/FFI/proc-macro/OS APIs that have no TS equivalent |
| **No TS equiv** | TS codebase has nothing comparable; this is a cocode-rs-only feature |
| **Rust superior** | Both exist, but cocode-rs implementation is materially better (with evidence) |
| **HYBRID** | Copy cocode-rs structure, but integrate TS logic where TS leads |

| Crate | Verdict | Reason |
|-------|---------|--------|
| `coco-error` + stack-trace-macro | KEEP | **Rust-only**: snafu + proc-macro are Rust idioms; TS has no structured error system |
| `coco-otel` | **HYBRID** | **Rust superior** L0-L1 (export 管道 + 7 基础事件)，但需从 TS 新增 L2 span 层级 (`sessionTracing.ts`)、L3 应用事件 (~53 种)、L4 业务 metrics (token/cost/LOC 等 8+)、L5 自定义 exporter (BigQuery/1P/Perfetto)。L6 运营控制 (sampling/killswitch) 暂不实现。详见 `crate-coco-otel.md` |
| `vercel-ai/*` (8 crates) | KEEP | **No TS equiv**: TS uses `@anthropic-ai/sdk` only; vercel-ai is cocode-rs's unique multi-provider advantage |
| `utils/shell-parser` | **HYBRID** | cocode-rs has stronger security analysis (24 risk types); TS has corpus-validated parser (3.4K inputs). Merge both. |
| `utils/apply-patch` | KEEP | **No TS equiv**: TS has no standalone patch engine |
| `utils/file-ignore` | KEEP | **Rust superior**: native pattern matching via ripgrep `ignore` crate; TS shells out to `git check-ignore` (~100 LOC) |
| `utils/file-search` | KEEP | **No TS equiv**: TS has no indexed fuzzy search with Nucleo scoring |
| `utils/secret-redact` | KEEP | **Rust superior**: explicit multi-provider regex patterns, zero-copy; TS has minimal equivalent |
| `utils/pty` | KEEP | **Rust-only**: pseudo-terminal requires libc FFI |
| `utils/keyring-store` | KEEP | **Rust-only**: OS keyring integration via platform APIs |
| `utils/rustls-provider` | KEEP | **Rust-only**: TLS provider for reqwest |
| Other 13 utils | KEEP | **Rust-only** or **No TS equiv**: cargo-bin, cache, image, git, common, async-utils, etc. are Rust infrastructure |
| `exec/sandbox` | KEEP | **Rust superior**: seccomp filtering, Seatbelt/bubblewrap enforcement, violation ring buffer; TS is just adapter around npm package |
| `exec/process-hardening` | KEEP | **Rust-only**: `prctl`, `ptrace(PT_DENY_ATTACH)`, env sanitization via libc; TS has nothing |
| `coco-lsp` | KEEP | **Rust superior**: AI-friendly symbol-name queries (not line/column), built-in caching, incremental sync, auto-restart; TS's LSP is simpler |
| `coco-retrieval` | KEEP (optional) | **No TS equiv**: TS has no code search/retrieval system |
| `coco-shell` (exec/shell) | **HYBRID** | Build on cocode-rs `utils/shell-parser` (24 analyzers, native Rust parsing). Add TS enhancements: read-only validation (40 cmds + 200 flags), destructive warnings (18 patterns), 7-phase permission pipeline, two-phase wrapper stripping (HackerOne fix), 3.4K-input test corpus. |

---

## 1. TS Module -> Rust Crate Mapping

### Layer 0: Foundation (from cocode-rs)

| cocode-rs crate | coco-rs crate | Notes |
|----------------|---------------|-------|
| `common/error` + `stack-trace-macro` | `coco-error` | snafu + StatusCode. Copy. |
| `common/otel` | `coco-otel` | **HYBRID**: 复用 L0-L1 (export + 基础事件), 新增 L2-L5 (span 层级 + 53 应用事件 + 8 业务 metrics + BigQuery/1P/Perfetto exporter)。详见 `crate-coco-otel.md` |
| All 24 `utils/*` | `utils/*` | Copy, rename `cocode-*` -> `coco-*` |

### Layer 0.5: AI Provider SDK (from cocode-rs, unique advantage)

| cocode-rs crate | coco-rs crate | Notes |
|----------------|---------------|-------|
| `vercel-ai/provider` | `vercel-ai-provider` | Copy. NO rename (independent library) |
| `vercel-ai/provider-utils` | `vercel-ai-provider-utils` | Copy |
| `vercel-ai/ai` | `vercel-ai` | Copy |
| `vercel-ai/openai` | `vercel-ai-openai` | Copy |
| `vercel-ai/openai-compatible` | `vercel-ai-openai-compatible` | Copy |
| `vercel-ai/google` | `vercel-ai-google` | Copy |
| `vercel-ai/anthropic` | `vercel-ai-anthropic` | Copy |
| `vercel-ai/bytedance` | `vercel-ai-bytedance` | Copy |

**provider-sdks/ REMOVED** — vercel-ai handles all provider abstraction.

### Layer 1: Types (TS `types/` + `Tool.ts` + `Task.ts`)

| TS source | Rust crate | Key types |
|-----------|-----------|-----------|
| `types/message.ts` | `coco-types` | `Message`, `UserMessage`, `AssistantMessage`, `SystemMessage`, `ToolResultMessage` |
| `types/permissions.ts` | `coco-types` | `PermissionMode`, `PermissionBehavior`, `PermissionRule` |
| `types/command.ts` | `coco-types` | `CommandBase`, `CommandType` |
| `types/hooks.ts` | `coco-types` | `HookEvent`, `HookEventType` |
| `types/ids.ts` | `coco-types` | `SessionId`, `AgentId`, `TaskId` |
| `types/plugin.ts` | `coco-types` | `PluginManifest`, `PluginError` |
| `Tool.ts` | `coco-types` | `ToolInputSchema`, `ToolResult<T>`, `ToolProgress<P>` |
| `Task.ts` | `coco-types` | `TaskType`, `TaskStatus`, `TaskStateBase`, `TaskHandle` |
| (from cocode-rs sandbox) | `coco-types` | `SandboxMode` (ReadOnly, WorkspaceWrite, FullAccess, ExternalSandbox) — needed by exec/sandbox |

### Layer 2: Config & Model (TS `utils/settings/` + `utils/model/`)

| TS source | Rust crate | Key types |
|-----------|-----------|-----------|
| `utils/settings/types.ts` | `coco-config` | `Settings` (Zod -> serde validation), layered loading |
| `utils/settings/settings.ts` | `coco-config` | `load_settings()`, `watch_settings()` |
| `utils/model/model.ts` | `coco-config` | `get_main_loop_model()`, model selection logic |
| `utils/model/configs.ts` | `coco-config` | `ModelConfig` (per-provider model IDs) |
| `utils/model/aliases.ts` | `coco-config` | `ModelAlias` (sonnet, opus, haiku, best) |
| `utils/model/providers.ts` | `coco-config` | `ProviderApi` enum (Anthropic, Openai, Gemini, etc.) + provider detection |
| `utils/model/modelCapabilities.ts` | `coco-config` | `ModelCapability`, capability detection |
| `utils/effort.ts` | `coco-config` | ThinkingLevel support checks (TS EffortLevel → unified into ThinkingLevel in coco-types) |
| `utils/fastMode.ts` | `coco-config` | `FastModeState`, availability checks |
| `constants/*` | `coco-config` | System constants |
| `migrations/` | `coco-config` | Settings migrations (model renames, config format upgrades) |
| `services/remoteManagedSettings/` | `coco-config` | Remote settings sync (enterprise/managed) |
| `services/settingsSync/` | `coco-config` | Settings synchronization |

**Key design**: TS separates model config from general config. In Rust, consolidate into `coco-config` with submodules `config::settings`, `config::model`, `config::constants`, `config::migrations`.

**Redesign note**: cocode-rs had `ModelInfo` in protocol. New design follows TS's `ModelConfig` + `ModelAlias` + `ModelCapability` pattern, but extended for multi-provider via vercel-ai.

### Layer 3: Core Services

| TS source | Rust crate | What it does |
|-----------|-----------|-------------|
| `services/api/claude.ts` | `coco-inference` | LLM API client. TS uses `@anthropic-ai/sdk`; Rust uses vercel-ai multi-provider |
| `services/api/client.ts` | `coco-inference` | Client factory, auth handling |
| `services/api/withRetry.ts` | `coco-inference` | Retry with exponential backoff, fallback |
| `services/api/errors.ts` | `coco-inference` | Error classification (retryable, auth, rate limit) |
| `services/api/logging.ts` | `coco-inference` | Request/response logging |
| `services/api/usage.ts` | `coco-inference` | Token usage accumulation |
| `utils/auth.ts` | `coco-inference` | Authentication (API key, OAuth, Bedrock, Vertex) |
| `services/tokenEstimation.ts` | `coco-inference` | Token counting/estimation for budget tracking |
| `services/oauth/` | `coco-inference` | OAuth 2.0 authentication flows |
| `services/claudeAiLimits.ts` | `coco-inference` | Rate limit enforcement and messaging |
| `services/rateLimitMessages.ts` | `coco-inference` | Rate limit user-facing messages |
| `services/policyLimits/` | `coco-inference` | Organization policy enforcement |
| `services/analytics/` | `coco-otel` | Telemetry events, 1P logging, BigQuery exporter, analytics sink (~4K LOC)。GrowthBook/killswitch 暂不实现 (L6) |
| | | |
| `services/mcp/client.ts` | `coco-mcp` | MCP server discovery, connection lifecycle |
| `services/mcp/types.ts` | `coco-mcp` | `MCPServerConnection`, `Transport`, `ConfigScope` |
| `services/mcp/config.ts` | `coco-mcp` | MCP server configuration |
| `services/mcp/auth.ts` | `coco-mcp` | OAuth for MCP servers |
| | | |
| `services/lsp/` | `coco-lsp` | LSP integration (copy+refactor from cocode-rs, superior AI-friendly API) |

### Layer 4: Messages & Context

| TS source | Rust crate | What it does |
|-----------|-----------|-------------|
| `utils/messages.ts` | `coco-messages` | Message creation, normalization, filtering |
| `history.ts` | `coco-messages` | Session history persistence |
| `cost-tracker.ts` | `coco-messages` | Token usage, cost tracking per session |
| | | |
| `context.ts` | `coco-context` | System context injection (git status, cwd, env info) |
| `utils/systemPromptType.ts` | `coco-context` | System prompt building |
| `utils/claudemd.ts` | `coco-context` | CLAUDE.md discovery and loading |
| `utils/attachments.ts` (4K lines) | `coco-context` | Attachment system: files, PDFs, memories, hooks, teammate mailbox |
| `utils/cwd.ts` | `coco-context` | Working directory management |
| `services/AgentSummary/` | `coco-context` | Agent activity summary for context |
| | | |
| `utils/permissions/` | `coco-permissions` | Permission rule evaluation, denial tracking |
| `types/permissions.ts` | `coco-permissions` | Rule-based permission system |
| | | |
| `services/compact/compact.ts` | `coco-compact` | Full compaction algorithm |
| `services/compact/microCompact.ts` | `coco-compact` | Lightweight compaction |
| `services/compact/autoCompact.ts` | `coco-compact` | Auto-trigger on context threshold |
| `services/compact/sessionMemoryCompact.ts` | `coco-compact` | Session memory compaction |

### Layer 5: Tool System

| TS source | Rust crate | What it does |
|-----------|-----------|-------------|
| `Tool.ts` | `coco-tool` | `Tool` trait (Rust version of TS interface) |
| `services/tools/StreamingToolExecutor.ts` | `coco-tool` | Concurrent tool execution (safe vs queued) |
| `services/tools/toolOrchestration.ts` | `coco-tool` | Tool dispatch, result processing |
| `tools.ts` | `coco-tool` | Tool registry, feature-gated loading |
| | | |
| `tools/BashTool/` | `coco-tools` | Shell execution tool (largest tool) |
| `tools/FileReadTool/` | `coco-tools` | File reading (images, PDFs, notebooks) |
| `tools/FileWriteTool/` | `coco-tools` | File creation/overwrite |
| `tools/FileEditTool/` | `coco-tools` | Search-replace editing |
| `tools/GlobTool/` | `coco-tools` | File pattern matching |
| `tools/GrepTool/` | `coco-tools` | Content search |
| `tools/WebFetchTool/` | `coco-tools` | URL content fetching |
| `tools/WebSearchTool/` | `coco-tools` | Web search |
| `tools/AgentTool/` | `coco-tools` | Subagent spawning |
| `tools/SkillTool/` | `coco-tools` | Skill invocation |
| `tools/NotebookEditTool/` | `coco-tools` | Jupyter notebook editing |
| `tools/AskUserQuestionTool/` | `coco-tools` | Interactive user prompts |
| `tools/EnterPlanModeTool/` | `coco-tools` | Plan mode entry |
| `tools/ExitPlanModeTool/` | `coco-tools` | Plan mode exit |
| `tools/EnterWorktreeTool/` | `coco-tools` | Git worktree isolation |
| `tools/ExitWorktreeTool/` | `coco-tools` | Git worktree exit |
| `tools/LSPTool/` | `coco-tools` | Language server integration |
| `tools/ConfigTool/` | `coco-tools` | Config management |
| `tools/ToolSearchTool/` | `coco-tools` | Deferred tool discovery |
| `tools/TaskCreate/Update/Get/List/Stop/Output` | `coco-tools` | Task management tools |
| `tools/TodoWriteTool/` | `coco-tools` | TODO management |
| `tools/SendMessageTool/` | `coco-tools` | Inter-agent messaging |
| `tools/TeamCreate/DeleteTool/` | `coco-tools` | Team management |
| `tools/ListMcpResourcesTool/` | `coco-tools` | MCP resource listing |
| `tools/ReadMcpResourceTool/` | `coco-tools` | MCP resource reading |
| `tools/ScheduleCronTool/` | `coco-tools` | Cron scheduling |
| `tools/RemoteTriggerTool/` | `coco-tools` | Remote trigger management |
| `tools/BriefTool/` | `coco-tools` | Summary generation |

**Design**: Split into `coco-tool` (trait + executor + registry) and `coco-tools` (all implementations). This follows the TS split of `Tool.ts` + `services/tools/` vs `tools/`.

### Root Modules (TS top-level peers)

| TS source | Rust crate | Directory | What it does |
|-----------|-----------|-----------|-------------|
| `commands.ts` + `commands/` | `coco-commands` | `commands/` | Slash command registry + ~30 implementations |
| `skills/` | `coco-skills` | `skills/` | Markdown workflow loading, skill discovery |
| `schemas/hooks.ts` + `utils/hooks/` | `coco-hooks` | `hooks/` | Hook definitions (pre/post tool, session), execution |
| `tasks/` + `Task.ts` | `coco-tasks` | `tasks/` | Background task system (LocalShell, LocalAgent, Workflow) |
| `memdir/` + `services/extractMemories/` + `services/SessionMemory/` + `services/autoDream/` | `coco-memory` | `memory/` | CLAUDE.md management, auto-extraction, session memory |
| `plugins/` + `services/plugins/` | `coco-plugins` | `plugins/` | Plugin manifest, loading, lifecycle |
| `keybindings/` | `coco-keybindings` | `keybindings/` | Keyboard shortcut management |

### app/ group: Query Engine + Application

| TS source | Rust crate | Directory | What it does |
|-----------|-----------|-----------|-------------|
| `query.ts` + `QueryEngine.ts` + `query/` | `coco-query` | `app/query/` | Multi-turn agent loop, single-turn execution, token budget |
| `state/AppState.ts` + `AppStateStore.ts` | `coco-state` | `app/state/` | App state tree (`Arc<RwLock<AppState>>`) |
| `bootstrap/state.ts` + session mgmt | `coco-session` | `app/session/` | Session init, persistence, resume |
| `components/` + `screens/` + `ink/` + `outputStyles/` + `services/notifier.ts` | `coco-tui` | `app/tui/` | Terminal UI (ratatui TEA) |
| `entrypoints/` + `main.tsx` + `cli/` + `server/` | `coco-cli` | `app/cli/` | CLI entry (clap), transports (SSE, WS, NDJSON), server/daemon mode |

### Standalone

| TS source | Rust crate | Directory | What it does |
|-----------|-----------|-----------|-------------|
| `bridge/` | `coco-bridge` | `bridge/` | IDE bridge (VS Code, JetBrains) |

### Layer 9: Exec

| Source | coco-rs crate | Strategy | Reason |
|--------|---------------|----------|--------|
| TS `utils/bash/` (12K LOC) + `utils/Shell.ts` + `utils/shell/` + `tools/BashTool/bashPermissions.ts` + `bashSecurity.ts` (23K total) | `coco-shell` | **HYBRID** | Build on cocode-rs `utils/shell-parser` (24 analyzers, native Rust parsing) as base. Add TS enhancements: read-only validation (40 cmds), destructive warnings (18 patterns), 7-phase permission pipeline, two-phase wrapper stripping, 3.4K-input test corpus. |
| cocode-rs `exec/sandbox` (8.4K LOC) | `coco-sandbox` | **KEEP cocode-rs** | TS is just adapter around npm package; cocode-rs has seccomp, Seatbelt/bubblewrap, violation monitoring |
| cocode-rs `exec/process-hardening` (212 LOC) | `coco-process-hardening` | **KEEP cocode-rs** | Rust-only: prctl, ptrace deny, env sanitization via libc. No TS equivalent. |

### Optional (from cocode-rs, no TS equivalent)

| cocode-rs source | coco-rs crate | Directory | Notes |
|-----------------|---------------|-----------|-------|
| `retrieval/` | `coco-retrieval` | `retrieval/` | Optional. BM25+vector search. Not in TS. |

### SKIP (not porting)

| TS source | Reason |
|-----------|--------|
| `buddy/` | Cosmetic pet animation (easter egg). Zero agent impact. |
| `moreright/` | Internal-only stub (25 LOC no-ops). Dead code in external builds. |
| `native-ts/` | TS-specific native bindings. No Rust equivalent needed. |

### v2 (affects architecture, port early)

| TS source | LOC | What it does |
|-----------|-----|-------------|
| `coordinator/` + `utils/swarm/` | 7K | Multi-worker orchestration, team management, 3 backends (in-process/tmux/iTerm2), permission mailbox. See `crate-coco-coordinator.md` |
| `vim/` | 1.5K | Full Vim state machine (10 states, motions, operators, text objects, dot-repeat). See `crate-coco-vim.md` |
| `assistant/` | 337 | Session history pagination API (cursor-based, viewport-aware loading). See `crate-coco-assistant.md` |
| `voice/` + `services/voice*.ts` | 1.9K | Voice recording (multi-platform fallback), WebSocket STT, keyterms, hold-to-talk. See `crate-coco-voice.md` |
| `services/compact/reactiveCompact.ts` | - | Reactive compaction (cache-aware). |

### v3 (conditional / niche, port later)

| TS source | LOC | What it does |
|-----------|-----|-------------|
| `remote/` + `upstreamproxy/` | 1.9K | CCR session management (WS + permission bridge + SDK message adapter) + upstream proxy (TCP CONNECT relay, credential injection). See `crate-coco-remote.md` |
| `utils/computerUse/` | 1.7K | Computer use tool (screen capture, mouse/keyboard). |
| `utils/claudeInChrome/` | 2K | Chrome browser integration. |
| `utils/nativeInstaller/` | 3K | Native binary installation. |
| `services/teamMemorySync/` | 2.2K | Team memory collaboration. |
| `services/PromptSuggestion/` | 1.5K | Prompt suggestion engine. |
| `services/tips/` | 761 | Tip/hint system. |
| `services/vcr.ts` | 406 | Recording/playback for testing. |

---

## 2. Crate Summary

| Group | Crates | Source | TS alignment |
|-------|--------|--------|--------------|
| `common/` | error, otel, types, config (4) | error+otel cocode-rs; types+config rewrite | Rust convention |
| `utils/` | 26 crates (24 cp + 2 new) | 24 cp + expand 5 + 2 new (`frontmatter`, `cursor`) | **matches TS `utils/`** — see `ts-utils-mapping.md` |
| `vercel-ai/` | 8 crates | cp from cocode-rs | Rust addition |
| `services/` | api, mcp, lsp, compact (4) | api+mcp+compact rewrite; lsp cocode-rs | **matches TS `services/`** |
| `core/` | messages, context, permissions, tool, tools (5) | all rewrite from TS | Rust convention |
| `exec/` | shell, sandbox, process-hardening (3) | shell rewrite; rest cocode-rs | Rust convention |
| root modules | commands, skills, hooks, tasks, memory, plugins, keybindings (7) | all rewrite from TS | **matches TS flat layout** |
| `app/` | query, state, session, tui, cli (5) | all rewrite from TS | Rust convention |
| standalone | bridge (1) | bridge rewrite from TS | **matches TS `bridge/`** |
| optional | retrieval (1) | cocode-rs | No TS equivalent |
| **v1 subtotal** | **64** (incl. cocode-rs copy) | | |
| v2 features | coordinator, vim, voice, assistant (4) | all rewrite from TS | **matches TS `coordinator/`, `vim/`, `voice/`, `assistant/`** |
| v3 features | remote (incl. proxy) (2) + ~7 TBD | rewrite from TS | **matches TS `remote/`, `upstreamproxy/`** |
| **Grand total** | **~77** | | |

**Note**: Analytics, rate limiting, OAuth, attachments, settings sync folded into existing crates (coco-inference, coco-context, coco-config).

---

## 3. Directory Structure

### Organization Principles

1. **Max 2-level nesting**: `group/crate/` — proven at scale in cocode-rs (81 crates)
2. **No single-crate directories**: every group has 2+ crates, otherwise standalone at root
3. **TS-first naming**: group names should match TS source directory names where possible
4. **Explicit workspace members**: no glob patterns (Cargo.toml lists each path)

### TS Source Layout Analysis

TS is essentially **flat** — 35 top-level dirs. The only grouped directory is `services/` (20 subdirs: api, mcp, lsp, compact, tools orchestration, analytics, oauth, etc.).

The rest (`commands/`, `skills/`, `tasks/`, `plugins/`, `memdir/`, `keybindings/`, `tools/`, `state/`, `query/`, `bridge/`) are all **top-level peers**.

### Naming decisions

| Rust group | TS match | Decision |
|---|---|---|
| `common/` | No TS equivalent | **Rust convention** — foundation layer has no TS analog |
| `utils/` | TS `utils/` | **Direct match** |
| `vercel-ai/` | No TS equivalent | **Rust addition** — multi-provider SDK |
| `services/` | TS `services/` | **Direct match** — includes compact (TS `services/compact/`) |
| `tools/` | TS `tools/` | **Direct match** — tool implementations |
| `exec/` | No TS equivalent | **Rust convention** — shell/sandbox are Rust-specific |
| ~~`features/`~~ | **No TS equivalent** | **Removed** — "features" is a cocode-rs-ism. In TS these are top-level modules. |
| `app/` | No TS group | **Rust convention** — application entry points |

### Structure

```
coco-rs/
  Cargo.toml
  rust-toolchain.toml  rustfmt.toml  clippy.toml  deny.toml  justfile
  .cargo/config.toml

  # ─── common/ (4) ── Rust foundation, no TS equivalent ───
  common/
    error/              # coco-error: snafu + StatusCode
    otel/               # coco-otel: OpenTelemetry + analytics
    types/              # coco-types: Message, Tool, Task, Permission types
    config/             # coco-config: settings, model, effort, fast mode

  # ─── utils/ (26) ── matches TS utils/, cp from cocode-rs + 2 new ───
  utils/
    # Copied from cocode-rs (24):
    absolute-path/  apply-patch/  async-utils/  cache/
    cargo-bin/  common/  file-encoding/  file-ignore/
    file-search/  file-watch/  git/  image/
    json-to-toml/  keyring-store/  pty/  readiness/
    rustls-provider/  secret-redact/  shell-parser/  sleep-inhibitor/
    stdio-to-uds/  stream-parser/  string/  symbol-search/
    # New (2, from TS utils that are generic infrastructure):
    frontmatter/    # YAML frontmatter parser (from TS frontmatterParser.ts, json.ts, markdown.ts)
    cursor/         # TUI input cursor with kill ring (from TS Cursor.ts, 1530 LOC)

  # ─── vercel-ai/ (8) ── Rust addition, no TS equivalent ───
  vercel-ai/
    provider/  provider-utils/  ai/
    openai/  openai-compatible/  google/  anthropic/  bytedance/

  # ─── services/ (4) ── matches TS services/ ───
  services/
    inference/          # coco-inference    <- TS services/api/ + oauth/ + analytics/ + rate limiting
    mcp/                # coco-mcp    <- TS services/mcp/
    lsp/                # coco-lsp    <- TS services/lsp/ (from cocode-rs)
    compact/            # coco-compact <- TS services/compact/ (TS puts this under services/)

  # ─── core/ (5) ── messages + context + permissions + tool engine ───
  core/
    messages/           # coco-messages     <- TS utils/messages.ts + history.ts + cost-tracker.ts
    context/            # coco-context      <- TS context.ts + attachments.ts + claudemd.ts
    permissions/        # coco-permissions  <- TS utils/permissions/
    tool/               # coco-tool         <- TS Tool.ts + services/tools/ (trait + executor + registry)
    tools/              # coco-tools        <- TS tools/ (40+ implementations)

  # ─── exec/ (3) ── Rust-specific execution layer ───
  exec/
    shell/              # coco-shell              <- HYBRID: cocode-rs shell-parser base + TS enhancements
    sandbox/            # coco-sandbox            <- cocode-rs (KEEP)
    process-hardening/  # coco-process-hardening  <- cocode-rs (KEEP)

  # ─── Top-level TS modules (7) ── matches TS flat layout ───
  commands/             # coco-commands     <- TS commands/ (standalone, like TS)
  skills/               # coco-skills       <- TS skills/
  hooks/                # coco-hooks        <- TS schemas/hooks.ts + utils/hooks/
  tasks/                # coco-tasks        <- TS tasks/
  memory/               # coco-memory       <- TS memdir/ + services/extractMemories/ + SessionMemory/
  plugins/              # coco-plugins      <- TS plugins/ + services/plugins/
  keybindings/          # coco-keybindings  <- TS keybindings/

  # ─── app/ (5) ── application entry points ───
  app/
    query/              # coco-query    <- TS QueryEngine.ts + query.ts
    state/              # coco-state    <- TS state/
    session/            # coco-session  <- TS bootstrap/ + session management
    tui/                # coco-tui      <- TS components/ + screens/ + ink/
    cli/                # coco-cli      <- TS entrypoints/ + main.tsx + cli/

  # ─── standalone (2) ───
  bridge/               # coco-bridge    <- TS bridge/
  retrieval/            # coco-retrieval <- cocode-rs (optional, no TS equivalent)
```

### Group Sizes

| Group | Crates | TS alignment |
|-------|--------|-------------|
| `common/` | 4 | Rust convention (no TS equivalent) |
| `utils/` | 24 | **Matches TS `utils/`** |
| `vercel-ai/` | 8 | Rust addition |
| `services/` | 4 | **Matches TS `services/`** (api, mcp, lsp, compact) |
| `core/` | 5 | Rust convention (TS scatters these) |
| `exec/` | 3 | Rust convention (shell/sandbox) |
| Top-level modules | 7 | **Matches TS flat layout** (commands, skills, hooks, tasks, memory, plugins, keybindings) |
| `app/` | 5 | Rust convention (query, state, session, tui, cli) |
| standalone | 2 | bridge (TS top-level), retrieval (cocode-rs) |
| **Total** | **62** | |

### Key decisions explained

- **`features/` removed**: "features" was a cocode-rs term for feature-gated modules. TS has commands/, skills/, tasks/ etc. as **top-level peers**. We follow TS — they're standalone crates at root.
- **`compact/` moved to `services/`**: TS puts `compact/` under `services/`, not standalone. It's a service that calls the LLM.
- **`common/` for types + config**: these are foundational and shared by all layers. TS has `types/` at root, but in Rust they need to be in a group (otherwise single-crate-dir). `common/` is the standard Rust name for shared foundation.
- **7 crates at root**: commands, skills, hooks, tasks, memory, plugins, keybindings — matching TS's flat layout. Each is a standalone crate (not in a group), because they're first-class modules in TS, not subordinate to any grouping.

---

## 4. Key Design Decisions

### 4.1 Model Configuration (redesigned)

> **Note**: Code snippets in section 4 are illustrative overviews. For authoritative definitions, see the respective `crate-coco-*.md` docs and `CLAUDE.md` canonical names.

TS design to follow:
```rust
// coco-config/src/model/mod.rs — see crate-coco-config.md for authoritative version

pub enum ProviderApi {  // canonical name (not ApiProvider)
    FirstParty,      // Anthropic direct
    Bedrock,         // AWS Bedrock
    Vertex,          // GCP Vertex
    Foundry,         // Azure Foundry
    // Extended for vercel-ai multi-provider:
    OpenAI,
    Google,
    OpenAICompatible { base_url: String },
}

pub enum ModelAlias {
    Sonnet,
    Opus,
    Haiku,
    Best,           // -> Opus
    SonnetLargeCtx, // sonnet[1m]
    OpusLargeCtx,   // opus[1m]
}

// EffortLevel removed — unified into ThinkingLevel (coco-types).
// See crate-coco-types.md for ThinkingLevel struct definition.

pub struct ModelConfig {
    pub canonical_name: String,
    pub provider_ids: HashMap<ProviderApi, String>,  // per-provider model ID
    pub max_input_tokens: i64,
    pub max_output_tokens: i64,
    pub supports_thinking: bool,
    pub supports_vision: bool,
    pub supports_tool_use: bool,
    pub supports_effort: bool,
    pub supports_fast_mode: bool,
}

pub fn get_main_loop_model(settings: &Settings, overrides: &RuntimeOverrides) -> String;
pub fn resolve_alias(alias: ModelAlias, provider: ProviderApi) -> String;
```

### 4.2 Tool Trait (Rust best practices + TS interface)

```rust
// coco-tool/src/trait.rs

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self, input: &Value) -> String;
    fn input_schema(&self) -> &ToolInputSchema;

    // TS interface methods:
    fn is_enabled(&self) -> bool { true }
    fn is_read_only(&self, input: &Value) -> bool { false }
    fn is_concurrency_safe(&self, input: &Value) -> bool { false }
    fn should_defer(&self) -> bool { false }

    // Execution (maps to TS call())
    async fn execute(
        &self,
        input: Value,
        context: &ToolUseContext,
        cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError>;

    // Permission check (TS checkPermissions)
    async fn check_permissions(  // canonical: plural
        &self,
        input: &Value,
        context: &ToolUseContext,
    ) -> PermissionDecision { PermissionDecision::Allow { .. } }
}
```

### 4.3 Query Engine (follows TS flow)

```rust
// coco-query/src/engine.rs  <- TS QueryEngine.ts

pub struct QueryEngine {
    config: QueryConfig,
    tool_registry: Arc<ToolRegistry>,
    command_registry: Arc<CommandRegistry>,
    api_client: Arc<ApiClient>,  // wraps vercel-ai LanguageModelV4 (see crate-coco-query.md)
    state: Arc<RwLock<AppState>>,
}

impl QueryEngine {
    /// Multi-turn conversation loop.
    /// Maps to TS QueryEngine.executeQuery()
    pub async fn run(
        &self,
        messages: &mut Vec<Message>,
        cancel: CancellationToken,
    ) -> Result<(), QueryError> {
        loop {
            // 1. Build system prompt (context.ts)
            // 2. Normalize messages for API (utils/messages.ts)
            // 3. Call LLM via vercel-ai (services/api/)
            // 4. Stream response, accumulate tool calls
            // 5. Execute tools (services/tools/StreamingToolExecutor)
            // 6. Check stop conditions
            // 7. Auto-compact if needed (services/compact/)
            // 8. Loop if tool calls, else done
        }
    }
}
```

### 4.4 AppState (follows TS Zustand pattern)

```rust
// coco-state/src/lib.rs  <- TS state/AppState.ts

pub struct AppState {
    // Config
    pub settings: Settings,
    pub main_loop_model: String,

    // MCP
    pub mcp_clients: Vec<McpConnection>,
    pub mcp_tools: Vec<Arc<dyn Tool>>,

    // Plugins
    pub plugins: PluginState,

    // Permissions
    pub permission_context: ToolPermissionContext,

    // Tasks
    pub tasks: HashMap<TaskId, TaskState>,

    // Agents
    pub agent_name_registry: HashMap<String, AgentId>,
}
```

### 4.5 Error Handling (Rust best practice from cocode-rs)

Each crate defines errors with snafu:
```rust
#[derive(Snafu, Debug)]
#[stack_trace_debug]
pub enum QueryError {
    #[snafu(display("API call failed"))]
    ApiError { source: ApiError },
    #[snafu(display("tool execution failed: {}", tool_id.as_wire_str()))]
    ToolError { tool_id: ToolId, source: ToolError },
    #[snafu(display("context overflow"))]
    ContextOverflow,
}
```

---

## 5. Dependency Graph

### Layered dependency direction (lower never imports higher)

```
┌─────────────────────────────────────────────────────────────┐
│ app/cli ── app/tui ── app/session                           │
│     \         |          /                                   │
│      └── app/query ─────┘     bridge/                        │
│            |    |    \                                        │
│       coco-state  \   coco-compact ──► coco-inference              │
├─────────────────────────────────────────────────────────────┤
│ commands/  skills/  hooks/  tasks/  memory/  plugins/        │
│     |        |        |       |        |        |            │
│     ├────────┴──┬─────┘       |     coco-inference    |            │
│     |           |             |                  |            │
├─────┴───────────┴─────────────┴──────────────────┴──────────┤
│ core/tools ──► core/tool (trait)                             │
│     |              |                                         │
│     ├── exec/shell + exec/sandbox                            │
│     ├── services/mcp                                         │
│     ├── services/lsp                                         │
│     └── core/permissions                                     │
│              |                                               │
│     core/messages    core/context                            │
├──────────────────────────────────────────────────────────────┤
│ services/api ──► vercel-ai/* (multi-provider)                │
├──────────────────────────────────────────────────────────────┤
│ common/config ──► common/types ──► common/error              │
│ common/otel                                                  │
├──────────────────────────────────────────────────────────────┤
│ utils/* (24 crates, independent)                             │
└──────────────────────────────────────────────────────────────┘
```

### Precise per-crate dependency table

| Crate | Depends on | Layer |
|-------|-----------|-------|
| `coco-types` | (nothing internal) | L1 |
| `coco-error` | (nothing internal) | L1 |
| `coco-otel` | coco-error | L1 |
| `coco-config` | coco-types, coco-error | L1 |
| `coco-inference` | coco-types, coco-config, coco-error, vercel-ai-* | L2 |
| `coco-sandbox` | coco-types (SandboxMode) | L2 |
| `coco-messages` | coco-types, coco-error | L3 |
| `coco-context` | coco-types, **coco-config** (ModelInfo), coco-error, utils/git | L3 |
| `coco-permissions` | coco-types, coco-config, coco-error | L3 |
| `coco-tool` | coco-types, **coco-config** (ModelInfo for tool filtering), coco-error | L3 |
| `coco-tools` | coco-tool, exec/shell, services/mcp, services/lsp, coco-permissions | L3 |
| `coco-shell` | coco-types, utils/shell-parser, coco-sandbox | L3 |
| `coco-compact` | coco-types, **coco-inference** (LLM for summary), coco-messages | L3 |
| `commands/` | coco-types, coco-tool (ToolRegistry) | L4 |
| `skills/` | coco-types, coco-config | L4 |
| `hooks/` | coco-types, coco-config | L4 |
| `tasks/` | coco-types, coco-tool | L4 |
| `memory/` | coco-types, **coco-inference** (LLM for extraction) | L4 |
| `plugins/` | coco-types, skills/, hooks/ | L4 |
| `coco-state` | coco-types, coco-config, coco-tool | L5 |
| `coco-query` | coco-types, coco-config, coco-inference, coco-tool, coco-context, coco-messages, coco-compact, coco-permissions, hooks/, coco-state | L5 |
| `coco-cli` | **everything** (top-level wiring) | L5 |

### Circular dependency prevention

```
core/tools depends on:           core/tools does NOT depend on:
  ✓ core/tool (trait, L3)          ✗ commands/ (callback injection)
  ✓ exec/shell (L3)               ✗ skills/ (callback injection)
  ✓ services/mcp (L2)             ✗ tasks/ (callback injection)
  ✓ services/lsp (L2)             ✗ memory/ (no direct dep)
  ✓ core/permissions (L3)         ✗ plugins/ (no direct dep)

SkillTool, TaskCreateTool etc. use ToolUseContext callbacks,
not direct crate imports. app/query wires these at runtime.
```

---

## 6. Phased Implementation

### Phase 1: Foundation
1. Create `coco-rs/` directory
2. Copy config files (rust-toolchain.toml, etc.)
3. Copy `utils/*` (24 crates), rename `cocode-*` -> `coco-*`
4. Copy `vercel-ai/*` (8 crates, no rename)
5. Copy `common/error` + `common/otel`, rename. Add L2 span 层级 (`SpanManager`: interaction→llm_request→tool→hook→user_input)
6. Copy `exec/sandbox`, `exec/process-hardening`, `lsp/`
7. Create workspace `Cargo.toml` + `justfile`
8. **Fix sandbox dep**: Add `SandboxMode` enum to `coco-types`, update `exec/sandbox/Cargo.toml` to depend on `coco-types` instead of `cocode-protocol`
9. **Fix shell dep**: Remove unused `cocode-protocol` dep from `exec/shell/Cargo.toml`
10. **Verify**: `cargo check`

### Phase 2: Types + Config
1. Create `coco-types` — translate TS `types/` to Rust enums/structs
2. Create `coco-config` — translate TS `utils/settings/` + `utils/model/` to Rust
3. **Verify**: `cargo test -p coco-types -p coco-config`

### Phase 3: Core Services
1. Create `coco-inference` — LLM client via vercel-ai (translate TS `services/api/`)
2. Create `coco-messages` — translate TS message types + history
3. Create `coco-context` — translate TS `context.ts`
4. Create `coco-permissions` — translate TS `utils/permissions/`
5. Create `coco-compact` — translate TS `services/compact/`
6. Copy+refactor `coco-mcp` (from cocode-rs rmcp-client, adapt to TS MCP patterns)
7. Copy+refactor `coco-lsp` (from cocode-rs)
8. Expand `coco-otel` L3-L5: 应用事件 (~53 种), 业务 metrics (8+), BigQuery/1P/Perfetto exporter
9. **Verify**: integration test with mock provider

### Phase 4: Tool System
1. Create `coco-tool` — Tool trait, ToolContext, executor, registry
2. **Rewrite `coco-shell` from TS** — translate TS `utils/bash/` (12K LOC) + `bashPermissions.ts` + `bashSecurity.ts` + `readOnlyValidation.ts` + command semantics. This is where TS leads.
3. Copy `coco-sandbox` from cocode-rs (Rust superior: seccomp, platform sandboxing)
4. Create `coco-tools` — all 40+ tool implementations
5. **Verify**: tool execution test (Bash, Read, Write, Edit)

### Phase 5: Features
1. Create `coco-commands` — Command trait + ~30 implementations
2. Create `coco-skills` — markdown workflow loading
3. Create `coco-hooks` — hook system
4. Create `coco-tasks` — background task system
5. Create `coco-memory` — CLAUDE.md + auto-extraction
6. Create `coco-plugins` — plugin system
7. **Verify**: command + skill + hook lifecycle tests

### Phase 6: Query Engine
1. Create `coco-query` — the agent loop, multi-turn orchestration
2. Wire all components together
3. **Verify**: end-to-end turn with real API call

### Phase 7: Application
1. Create `coco-state` — AppState tree
2. Create `coco-session` — session management
3. Create `coco-tui` — terminal UI (ratatui TEA)
4. Create `coco-cli` — binary entry point
5. **Verify**: `cargo run --bin coco -- -p "hello"`

---

## 7. What to Copy vs Rewrite

### KEEP cocode-rs (Rust-only / No TS equiv / Rust superior)
- `common/error` + `stack-trace-macro` — **Rust-only**: snafu + proc-macro idioms
- All 24 `utils/*` crates — **Rust-only** infrastructure or **No TS equiv**
- All 8 `vercel-ai/*` crates — **No TS equiv**: multi-provider SDK
- `exec/sandbox` — **Rust superior**: seccomp, Seatbelt; TS is just npm adapter
- `exec/process-hardening` — **Rust-only**: libc FFI, no TS equivalent
- `lsp/` — **Rust superior**: AI-friendly symbol queries, caching, incremental sync

### HYBRID (cocode-rs structure + TS logic)
- `coco-otel` — 复用 L0-L1 (export + 7 基础事件), 新增 L2-L5 (span 层级/53 应用事件/8 业务 metrics/BigQuery+1P+Perfetto). L6 运营控制暂不实现
- `utils/shell-parser` — cocode-rs security analysis + TS corpus-validated parser

### HYBRID (cocode-rs base + TS enhancements)
- `coco-shell` — cocode-rs `utils/shell-parser` (24 analyzers, native parsing) as base + TS read-only validation (40 cmds), destructive warnings, 7-phase permission pipeline, 3.4K test corpus

### REWRITE from TS (TS leads)
- `coco-types`, `coco-config` — redesigned from TS types/settings/model
- `coco-inference` — TS services/api through vercel-ai abstraction
- `coco-messages`, `coco-context`, `coco-permissions`, `coco-compact`
- `coco-tool`, `coco-tools` — TS Tool.ts + tools/ + services/tools/
- `coco-commands`, `coco-skills`, `coco-hooks`, `coco-tasks`, `coco-memory`, `coco-plugins`
- `coco-query` — TS QueryEngine.ts + query.ts
- `coco-state`, `coco-session`, `coco-tui`, `coco-cli`, `coco-bridge`

### Removed
- `provider-sdks/*` (6 crates) — replaced by vercel-ai
- `common/protocol` — replaced by `coco-types` (TS-first redesign)
- `common/config` — replaced by `coco-config` (TS-first redesign)
- `common/policy` — replaced by `coco-permissions`
- `core/*` (inference, message, tools-api, tools, context, prompt, system-reminder, loop, subagent, executor, file-backup) — all replaced by TS-mapped crates
- `features/*` (skill, hooks, plugin, plan-mode, team, auto-memory, cron, keybindings, ide, llm-check) — replaced by TS-mapped features
- `app/*` (session, cli, tui) — replaced by TS-mapped app crates
- `app-server`, `app-server-protocol` — replaced by `coco-bridge` (IDE bridge) + `coco-cli` (server mode)

---

## 8. Verification

| Phase | Test | Command |
|-------|------|---------|
| 1 | Foundation compiles | `cargo check` |
| 2 | Types + config tests | `cargo test -p coco-types -p coco-config` |
| 3 | Mock LLM integration | `cargo test -p coco-inference --test integration` |
| 4 | Tool execution | `cargo test -p coco-tool -p coco-tools` |
| 5 | Feature lifecycle | `cargo test -p coco-commands -p coco-hooks` |
| 6 | End-to-end turn | `cargo test -p coco-query --test e2e` |
| 7 | Binary smoke test | `cargo run --bin coco -- -p "hello"` |
| CI | Full suite | `just ci` (fmt + clippy + test + deny) |
