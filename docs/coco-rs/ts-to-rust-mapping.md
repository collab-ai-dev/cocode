# TS -> Rust Complete Mapping (Source of Truth)

Every TS source directory and top-level file mapped to its Rust crate, version, and strategy.

## Legend

- **Strategy**: `TS` = rewrite from TS | `cocode` = copy from cocode-rs | `HYBRID` = cocode-rs structure + TS logic | `SKIP` = not porting
- **Version**: `v1` = initial release | `v2` = second phase | `v3` = third phase | `SKIP` = never

---

## TS Directories

| TS dir | Rust crate | Rust dir | Version | Strategy | Notes |
|---------------|-----------|----------|---------|----------|-------|
| `types/` | `coco-types` | `common/types/` | v1 | TS | Message, Tool, Task, Permission, Command types |
| `constants/` | `coco-config` | `common/config/` | v1 | TS | System constants, merged into config |
| `utils/settings/` | `coco-config` | `common/config/` | v1 | TS | Settings schema (Zod -> serde), layered loading |
| `utils/model/` | `coco-config` | `common/config/` | v1 | TS | Model selection, aliases, providers, capabilities |
| `migrations/` | `coco-config` | `common/config/` | v1 | TS | Settings format migrations |
| `services/remoteManagedSettings/` | `coco-config` | `common/config/` | v1 | TS | Enterprise remote settings sync |
| `services/settingsSync/` | `coco-config` | `common/config/` | v1 | TS | Settings synchronization |
| `services/api/` | `coco-inference` | `services/inference/` | v1 | TS | LLM client via vercel-ai, retry, streaming |
| `utils/auth.ts` | `coco-inference` | `services/inference/` | v1 | TS | API key, OAuth, Bedrock/Vertex auth |
| `services/oauth/` | `coco-inference` | `services/inference/` | v1 | TS | OAuth 2.0 flows |
| `services/analytics/` (8 files) | `coco-otel` | `common/otel/` | v1 | HYBRID | 1P logging, BigQuery exporter, analytics sink (~4K LOC)。GrowthBook/killswitch = L6 暂不实现 |
| `services/tokenEstimation.ts` | `coco-inference` | `services/inference/` | v1 | TS | Token counting for budget |
| `services/claudeAiLimits.ts` | `coco-inference` | `services/inference/` | v1 | TS | Rate limit enforcement |
| `services/rateLimitMessages.ts` | `coco-inference` | `services/inference/` | v1 | TS | Rate limit user messages |
| `services/policyLimits/` | `coco-inference` | `services/inference/` | v1 | TS | Org policy enforcement |
| `services/mcp/` | `coco-mcp` | `services/mcp/` | v1 | TS | MCP server lifecycle, config, auth |
| `services/lsp/` | `coco-lsp` | `services/lsp/` | v1 | cocode | AI-friendly LSP (symbol-name queries, caching) |
| `services/compact/` | `coco-compact` | `services/compact/` | v1 | TS | Full/micro/auto/session compaction |
| `utils/messages.ts` | `coco-messages` | `core/messages/` | v1 | TS | Message creation, normalization |
| `history.ts` | `coco-messages` | `core/messages/` | v1 | TS | Session history persistence |
| `cost-tracker.ts` | `coco-messages` | `core/messages/` | v1 | TS | Token usage, cost tracking |
| `context.ts` | `coco-context` | `core/context/` | v1 | TS | System context (git, cwd, env) |
| `utils/systemPromptType.ts` | `coco-context` | `core/context/` | v1 | TS | System prompt building |
| `utils/claudemd.ts` | `coco-context` | `core/context/` | v1 | TS | CLAUDE.md discovery + loading |
| `utils/attachments.ts` | `coco-context` | `core/context/` | v1 | TS | Attachment system (4K LOC) |
| `utils/cwd.ts` | `coco-context` | `core/context/` | v1 | TS | Working directory management |
| `services/AgentSummary/` | `coco-context` | `core/context/` | v1 | TS | Agent activity summary |
| `utils/permissions/` | `coco-permissions` | `core/permissions/` | v1 | TS | Rule evaluation, denial tracking |
| `Tool.ts` | `coco-tool` | `core/tool/` | v1 | TS | Tool trait, ToolUseContext |
| `services/tools/` | `coco-tool` | `core/tool/` | v1 | TS | StreamingToolExecutor, orchestration |
| `tools.ts` | `coco-tool` | `core/tool/` | v1 | TS | Tool registry, feature gates |
| `tools/` (43 dirs) | `coco-tools` | `core/tools/` | v1 | TS | All built-in tool implementations (incl. PowerShellTool, REPLTool, McpAuthTool, SyntheticOutputTool) |
| `commands.ts` | `coco-commands` | `commands/` | v1 | TS | Command registry |
| `commands/` (~30 dirs) | `coco-commands` | `commands/` | v1 | TS | Slash command implementations |
| `skills/` | `coco-skills` | `skills/` | v1 | TS | Markdown workflow loading |
| `schemas/hooks.ts` | `coco-hooks` | `hooks/` | v1 | TS | Hook schema definitions |
| `utils/hooks/` (15+ files) | `coco-hooks` | `hooks/` | v1 | TS | Hook execution (bash/prompt/http/agent), fileChangedWatcher, postSamplingHooks, sessionHooks, ssrfGuard, skillImprovement |
| `tasks/` | `coco-tasks` | `tasks/` | v1 | TS | Background task system |
| `Task.ts` | `coco-tasks` | `tasks/` | v1 | TS | Task types (also in coco-types) |
| `memdir/` | `coco-memory` | `memory/` | v1 | TS | CLAUDE.md management |
| `services/extractMemories/` | `coco-memory` | `memory/` | v1 | TS | Auto-extraction |
| `services/SessionMemory/` | `coco-memory` | `memory/` | v1 | TS | Session memory persistence |
| `services/autoDream/` | `coco-memory` | `memory/` | v1 | TS | Memory consolidation |
| `plugins/` | `coco-plugins` | `plugins/` | v1 | TS | Plugin manifest, loading |
| `services/plugins/` | `coco-plugins` | `plugins/` | v1 | TS | Plugin lifecycle |
| `keybindings/` | `coco-keybindings` | `keybindings/` | v1 | TS | Keyboard shortcuts |
| `utils/processUserInput/` | `coco-query` | `app/query/` | v1 | TS | User input pre-processing (images, slash, bash) |
| `utils/fileHistory.ts` | `coco-context` | `core/context/` | v1 | TS | File edit tracking per turn |
| `utils/tokens.ts` | `coco-inference` | `services/inference/` | v1 | TS | Token extraction from messages/API |
| `utils/api.ts` (26K LOC) | `coco-inference` | `services/inference/` | v1 | TS | Tool schema conversion, CacheScope, system prompt blocks |
| `utils/modelCost.ts` | `coco-inference` | `services/inference/` | v1 | TS | Per-model pricing calculations |
| `utils/worktree.ts` | `coco-tools` | `core/tools/` | v1 | TS | Git worktree management |
| `utils/theme.ts` | `coco-tui` | `app/tui/` | v1 | TS | Theme management |
| `utils/toolResultStorage.ts` (Level 1: persist + preview) | `coco-tool-runtime` | `core/tool-runtime/src/tool_result_storage/` | v1 | TS | Per-tool persistence pipeline (`<persisted-output>` wrapper, 2KB preview, session-scoped `tool-results/`). See [`tool-result-budget-plan.md`](tool-result-budget-plan.md). |
| `utils/toolResultStorage.ts` (Level 2: budget) | `coco-query` (wiring) + `coco-tool-runtime` (state types) | `app/query/src/`, `core/tool-runtime/src/tool_result_storage/budget.rs` | v1 | TS | `ContentReplacementState` + `enforceToolResultBudget`, invoked from query loop before micro-compact. See [`tool-result-budget-plan.md`](tool-result-budget-plan.md). |
| `utils/toolResultStorage.ts` (transcript records) | `coco-session` | `app/session/src/storage.rs` | v1 | TS | `ContentReplacementRecord` persistence + resume reconstruction. |
| `constants/toolLimits.ts` | `coco-tool-runtime` | `core/tool-runtime/src/tool_result_storage/constants.rs` | v1 | TS | `DEFAULT_MAX_RESULT_SIZE_CHARS`, `PREVIEW_SIZE_BYTES`, `MAX_TOOL_RESULTS_PER_MESSAGE_CHARS`, etc. |
| `utils/mcpOutputStorage.ts` | `coco-tool-runtime` (Phase 3) | `core/tool-runtime/src/tool_result_storage/mcp_output.rs` | v1 | TS | Parallel pipeline for MCP server outputs; depends on Level 1 primitives. |
| `utils/fileStateCache.ts` (1.5K LOC) | `coco-context` | `core/context/` | v1 | TS | LRU file read cache per turn |
| `query.ts` | `coco-query` | `app/query/` | v1 | TS | Single-turn execution |
| `QueryEngine.ts` | `coco-query` | `app/query/` | v1 | TS | Multi-turn agent loop |
| `query/` | `coco-query` | `app/query/` | v1 | TS | Token budget, query config |
| `state/` | `coco-state` | `app/state/` | v1 | TS | AppState tree |
| `bootstrap/` | `coco-session` | `app/session/` | v1 | TS | Session init, persistence |
| `components/` | `coco-tui` | `app/tui/` | v1 | TS | Terminal UI components |
| `screens/` | `coco-tui` | `app/tui/` | v1 | TS | UI screens |
| `ink/` | `coco-tui` | `app/tui/` | v1 | TS | Terminal rendering |
| `outputStyles/` | `coco-tui` | `app/tui/` | v1 | TS | Output formatting |
| `services/notifier.ts` | `coco-tui` | `app/tui/` | v1 | TS | Notifications |
| `entrypoints/` | `coco-cli` | `app/cli/` | v1 | TS | CLI entry points |
| `main.tsx` | `coco-cli` | `app/cli/` | v1 | TS | Main entry |
| `cli/` | `coco-cli` | `app/cli/` | v1 | TS | Transports (SSE, WS, NDJSON) |
| `server/` | `coco-cli` | `app/cli/` | v1 | TS | Server/daemon mode |
| `bridge/` | `coco-bridge` | `bridge/` | v1 | TS | IDE bridge (VS Code, JetBrains) |
| `utils/bash/` | `coco-shell` | `exec/shell/` | v1 | HYBRID | Bash parsing + validation (12K LOC). Base: cocode-rs `utils/shell-parser` (24 analyzers). Add: TS read-only validation, destructive warnings, 7-phase permission pipeline |
| `utils/Shell.ts` | `coco-shell` | `exec/shell/` | v1 | HYBRID | Shell execution. Base: cocode-rs shell executor. Add: TS CWD tracking, env snapshotting |
| `utils/shell/` | `coco-shell` | `exec/shell/` | v1 | HYBRID | Shell utilities |
| `utils/effort.ts` | `coco-config` | `common/config/` | v1 | TS | Effort level |
| `utils/fastMode.ts` | `coco-config` | `common/config/` | v1 | TS | Fast mode |
| `utils/thinking.ts` | `coco-config` | `common/config/` | v1 | TS | Thinking/reasoning support |
| `utils/git.ts` | (uses `utils/git`) | `utils/git/` | v1 | cocode | Git operations |
| `coordinator/` | `coco-coordinator` | `coordinator/` | v2 | TS | Multi-worker orchestration |
| `vim/` | `coco-vim` | `app/tui/` (vim submodule) | v2 | TS | Full Vim state machine |
| `assistant/` | `coco-assistant` | `app/session/` (assistant submodule) | v2 | TS | Session history pagination API |
| `voice/` | `coco-voice` | `voice/` | v2 | TS | Voice mode gate + STT |
| `services/voice.ts` | `coco-voice` | `voice/` | v2 | TS | Voice mode gate |
| `utils/swarm/` | `coco-coordinator` | `coordinator/` | v2 | TS | Agent swarm (with coordinator) |
| `services/compact/reactiveCompact.ts` | `coco-compact` | `services/compact/` | v2 | TS | Reactive compaction |
| `remote/` | `coco-remote` | `remote/` | v3 | TS | CCR session management |
| `upstreamproxy/` | `coco-proxy` | `remote/` (proxy submodule) | v3 | TS | Enterprise proxy/MITM |
| `utils/computerUse/` | TBD | TBD | v3 | TS | Computer use tool |
| `utils/claudeInChrome/` | TBD | TBD | v3 | TS | Chrome integration |
| `utils/nativeInstaller/` | TBD | TBD | v3 | TS | Native binary install |
| `services/teamMemorySync/` | TBD | TBD | v3 | TS | Team memory collab |
| `services/PromptSuggestion/` | TBD | TBD | v3 | TS | Prompt suggestions |
| `services/tips/` | TBD | TBD | v3 | TS | Tip/hint system |
| `services/vcr.ts` | TBD | TBD | v3 | TS | Recording/playback |
| `services/awaySummary.ts` | `coco-context` | `core/context/` | v1 | TS | "While away" session recap (74 LOC) |
| `services/diagnosticTracking.ts` | `coco-lsp` | `services/lsp/` | v1 | TS | IDE diagnostics tracking via MCP (397 LOC) |
| `services/internalLogging.ts` | `coco-otel` | `common/otel/` | v1 | TS | Internal error logging to remote endpoint (90 LOC) |
| `services/mcpServerApproval.tsx` | `coco-tui` | `app/tui/` | v1 | TS | MCP server approval dialog (40 LOC) |
| `services/preventSleep.ts` | `coco-shell` | `exec/shell/` | v1 | TS | Prevent machine sleep during execution (165 LOC) |
| `services/claudeAiLimitsHook.ts` | `coco-tui` | `app/tui/` | v1 | TS | Rate limit UI hook (23 LOC) |
| `services/voiceKeyterms.ts` | `coco-voice` | `voice/` | v2 | TS | Voice mode keyword definitions (106 LOC) |
| `services/voiceStreamSTT.ts` | `coco-voice` | `voice/` | v2 | TS | Voice streaming speech-to-text (544 LOC) |
| `services/MagicDocs/` | `coco-context` | `core/context/` | v2 | TS | Auto-generated documentation (381 LOC) |
| `services/toolUseSummary/` | `coco-otel` | `common/otel/` | v2 | TS | Tool usage analytics (112 LOC) |
| `services/mockRateLimits.ts` | — | — | SKIP | — | Test fixture: rate limit mocking (882 LOC) |
| `services/rateLimitMocking.ts` | — | — | SKIP | — | Test fixture: rate limit test helpers (144 LOC) |
| `buddy/` | — | — | SKIP | — | Cosmetic pet animation |
| `moreright/` | — | — | SKIP | — | Internal-only stub (25 LOC) |
| `native-ts/` | — | — | SKIP | — | TS-specific native bindings |

## Previously Unmapped TS Utils Files (now assigned)

| TS file | Rust crate | Version | Strategy | Notes |
|---------|-----------|---------|----------|-------|
| `utils/envUtils.ts` | `coco-config` | v1 | TS | Config home dir, env helpers (is_env_truthy) |
| `utils/gitSettings.ts` | `coco-config` | v1 | TS | Git instruction inclusion setting |
| `utils/betas.ts` (434 LOC) | `coco-inference` | v1 | TS | **18 beta headers**, provider-specific capability branching |
| `utils/advisor.ts` | `coco-inference` | v1 | TS | Advisor tool types |
| `utils/editor.ts` | `coco-tools` | v1 | TS | External editor launching |
| `utils/glob.ts` | `coco-tools` | v1 | TS | Glob pattern matching for tools |
| `utils/path.ts` | `coco-tools` | v1 | TS | Path expansion (tilde, POSIX conversion) |
| `utils/platform.ts` | `coco-tools` | v1 | TS | Platform detection (macOS/Windows/Linux/WSL) |
| `utils/stream.ts` | `coco-tool` | v1 | TS | Async stream abstraction for tool results |
| `utils/lockfile.ts` | `coco-config` | v1 | TS | File locking for config writes |
| `utils/fsOperations.ts` (770 LOC) | `coco-tools` | v1 | TS | Abstracted FS operations interface |
| `utils/debug.ts` | `coco-otel` | v1 | TS | Debug logging with levels and filtering |
| `utils/cleanup.ts` (602 LOC) | `coco-session` | v1 | TS | Session cleanup (old caches, pastes, worktrees) |
| `utils/pasteStore.ts` | `coco-context` | v1 | TS | Paste content cache (hash-based) |
| `utils/releaseNotes.ts` | `coco-cli` | v1 | TS | Release notes fetching and display |
| `utils/terminal.ts` | `coco-tui` | v1 | TS | ANSI-aware text wrapping and slicing |
| `utils/thinking.ts` | `coco-inference` | v1 | TS | Provider-specific thinking config branching |
| `utils/filePersistence/` (2 files) | `coco-context` | v1 | TS | File persistence scanner + state tracking (413 LOC) |
| `utils/dxt/` (2 files) | `coco-tools` | v1 | TS | DXT image/compression helpers (314 LOC) |
| `utils/plugins/` (44 files) | `coco-plugins` | v1 | TS | Plugin system: manifest loading, marketplace, installation, lifecycle (20.5K LOC) |
| `utils/powershell/` (3 files) | `coco-shell` | v1 | TS | PowerShell parsing (66K parser.ts), static prefixes, dangerous cmdlets (2.3K LOC) |
| `utils/telemetry/` (9 files) | `coco-otel` | v1 | TS | Session/perfetto/beta tracing, instrumentation, bigquery export (4K LOC) |
| `utils/sandbox/` (2 files) | `coco-sandbox` | v1 | TS | Sandbox adapter for TS (35K sandbox-adapter), UI utils (1K LOC) |
| `utils/git/` (3 files) | `utils/git` + `coco-context` | v1 | TS | Git filesystem, config parser, gitignore parsing (1K LOC) |
| `utils/suggestions/` (5 files) | `coco-tui` + `coco-query` | v1 | TS | Command/directory/history/skill suggestions (1.2K LOC) |
| `utils/task/` (5 files) | `coco-tasks` | v1 | TS | Task framework, output persistence, disk output (1.2K LOC) |
| `utils/mcp/` (2 files) | `coco-mcp` | v1 | TS | MCP datetime parser, elicitation validation (457 LOC) |
| `utils/messages/` (2 files) | `coco-messages` | v1 | TS | Message mappers, system init helpers (386 LOC) |
| `utils/memory/` (2 files) | `coco-memory` | v1 | TS | Memory types and version constants (20 LOC) |
| `utils/secureStorage/` (6 files) | `utils/keyring-store` | v1 | TS | macOS Keychain, fallback storage, prefetch (629 LOC) |
| `utils/todo/` (1 file) | `coco-tasks` | v1 | TS | TodoV2 types (18 LOC) |
| `utils/skills/` (1 file) | `coco-skills` | v1 | TS | Skill change detector (311 LOC) |
| `utils/github/` (1 file) | `coco-tools` | v1 | TS | GitHub auth status check (29 LOC) |
| `utils/teleport/` (4 files) | `coco-session` | v2 | TS | Cross-machine session resume: api, git bundle, environments (955 LOC) |
| `utils/ultraplan/` (2 files) | `coco-query` | v2 | TS | CCR session, keyword routing for planning (476 LOC) |
| `utils/deepLink/` (6 files) | `coco-cli` | v2 | TS | Deep link parsing, terminal launch, protocol handler (1,388 LOC) |
| `utils/background/` | `coco-coordinator` | v2 | TS | Background job orchestration |

## Previously Unmapped Top-Level src/ Files (now assigned)

| TS file | Rust crate | Version | Strategy | Notes |
|---------|-----------|---------|----------|-------|
| `costHook.ts` | `coco-tui` | v1 | TS | Cost summary display hook |
| `dialogLaunchers.tsx` | `coco-tui` | v1 | TS | Dialog rendering (React-specific → TUI overlays) |
| `interactiveHelpers.tsx` | `coco-tui` | v1 | TS | renderAndRun, setup dialog |
| `projectOnboardingState.ts` | `coco-state` | v1 | TS | Onboarding step tracking |
| `replLauncher.tsx` | `coco-tui` | v1 | TS | REPL launch wrapper |
| `setup.ts` (477 LOC) | `coco-session` | v1 | TS | Setup flow orchestration (auth, settings init) |
| `ink.ts` | `coco-tui` | v1 | TS | Terminal renderer (React/Ink → ratatui) |

## TS Top-Level Files

| TS file | Rust crate | Version | Strategy | Notes |
|---------|-----------|---------|----------|-------|
| `Tool.ts` | `coco-types` + `coco-tool` | v1 | TS | Types in coco-types, trait in coco-tool |
| `Task.ts` | `coco-types` + `coco-tasks` | v1 | TS | Types in coco-types, manager in coco-tasks |
| `tools.ts` | `coco-tool` | v1 | TS | Tool registry |
| `commands.ts` | `coco-commands` | v1 | TS | Command registry |
| `tasks.ts` | `coco-tasks` | v1 | TS | Task factory |
| `query.ts` | `coco-query` | v1 | TS | Single-turn execution |
| `QueryEngine.ts` | `coco-query` | v1 | TS | Multi-turn loop |
| `context.ts` | `coco-context` | v1 | TS | System/user context |
| `history.ts` | `coco-messages` | v1 | TS | REPL history |
| `cost-tracker.ts` | `coco-messages` | v1 | TS | Cost tracking |

## TS React-Specific (no direct Rust equivalent)

| TS dir | What it is | Rust handling |
|--------|-----------|---------------|
| `hooks/` (80+ files) | React hooks (useCanUseTool, useMergedTools, etc.) | ~67 are pure UI wiring (no Rust port). **16 have core business logic** — see table below. |
| `context/` (9 files) | React contexts (mailbox, notifications, voice, stats) | State managed via `coco-state` (`Arc<RwLock<AppState>>`) |

### React Hooks with Business Logic (must port core logic)

| Hook | Core Logic | Target crate | Version |
|------|-----------|-------------|---------|
| `useTasksV2.ts` | Task list watcher + file sync | `coco-tasks` | v1 |
| `useFileHistorySnapshotInit.ts` | File edit tracking per turn | `coco-context` | v1 |
| `useIDEIntegration.tsx` | IDE bridge callbacks | `coco-bridge` | v1 |
| `useSwarmInitialization.ts` | Teammate context setup | `coco-coordinator` | v2 |
| `useSwarmPermissionPoller.ts` | Worker permission polling | `coco-permissions` | v2 |
| `useHistorySearch.ts` | History search + typeahead | `coco-query` | v2 |
| `useTeleportResume.tsx` | Cross-machine session resume | `coco-session` | v2 |
| `useScheduledTasks.ts` | Cron task scheduling | `coco-tasks` | v2 |
| `useVoiceIntegration.tsx` | Voice mode STT setup | `coco-voice` | v2 |
| `usePrStatus.ts` | GitHub PR status polling | `coco-tools` | v2 |
| `useQueueProcessor.ts` | Message queue processing | `coco-query` | v2 |
| `useBackgroundTaskNavigation.ts` | Background task UI nav | `coco-tui` | v2 |
| `useDiffInIDE.ts` | IDE diff viewer | `coco-bridge` | v2 |
| `useArrowKeyHistory.tsx` | Arrow key history nav | `coco-tui` | v2 |
| `useAssistantHistory.ts` | Assistant pagination | `coco-query` | v2 |
| `useVoiceEnabled.ts` | Voice feature gate logic | `coco-voice` | v2 |

## File-Level Detail: services/api/ (→ coco-inference)

All 20 files in `services/api/` map to `coco-inference`. Key files already documented in crate-coco-inference.md are marked; the rest need implementation coverage.

| File | LOC | Purpose | Doc Status |
|------|-----|---------|------------|
| `claude.ts` | 3,419 | Full query orchestration: streaming/non-streaming, beta headers, system prompt, fallback, media limits | Documented (audit R1-3) |
| `withRetry.ts` | 550 | Two-layer retry: exponential backoff + auth-aware + fast-mode aware + persistent mode | Documented (audit R1-3) |
| `filesApi.ts` | 748 | File upload/download: 500MB limit, retry, path security, concurrency pool | Documented (audit R1-3) |
| `dumpPrompts.ts` | 227 | Non-blocking debug trace: fingerprint dedup, JSONL output, session-scoped | Documented (audit R1-3) |
| `client.ts` | ~500 | API client construction, base URL resolution, header injection | Documented (crate-coco-inference.md) |
| `errors.ts` | ~300 | API error types, error classification, retryable detection | Documented (crate-coco-inference.md) |
| `bootstrap.ts` | ~400 | `/api/claude_cli/bootstrap` endpoint, 5s timeout, disk caching, model options merge | P1 gap |
| `usage.ts` | ~200 | Token usage extraction and aggregation from API responses | Documented implicitly |
| `logging.ts` | ~150 | Request/response logging for debug and telemetry | Documented implicitly |
| `promptCacheBreakDetection.ts` | ~727 | Prompt cache break detection + 2-phase detection | Documented (crate-coco-inference.md) |
| `sessionIngress.ts` | 514 | Session transcript logging and backend ingestion with retry, sequential per-session processing | **NEW** |
| `grove.ts` | 357 | Grove feature configuration, account settings caching, OAuth integration | **NEW** (v2/v3 feature) |
| `referral.ts` | 281 | Referral campaign eligibility checking and redemption tracking with OAuth and caching | **NEW** (v2/v3 feature) |
| `errorUtils.ts` | 260 | SSL/TLS error code constants, Anthropic SDK error handling utilities | **NEW** |
| `metricsOptOut.ts` | 159 | Metrics opt-out status: dual-layer caching (in-memory + disk) to minimize API calls | **NEW** |
| `overageCreditGrant.ts` | 137 | Overage credit grant eligibility by tier/role with 1h TTL cache | **NEW** (v2/v3 feature) |
| `adminRequests.ts` | 119 | Admin request creation (limit increases, seat upgrades) with typed request/response | **NEW** (v2/v3 feature) |
| `firstTokenDate.ts` | 60 | First Claude Code token date fetch + cache (post-login historical tracking) | **NEW** |
| `ultrareviewQuota.ts` | 38 | Ultrareview quota (reviews used/limit/remaining) for subscribers | **NEW** (v2/v3 feature) |
| `emptyUsage.ts` | 22 | Zero-initialized usage constant (avoids circular imports) | **NEW** |

**v1 implementation**: claude.ts, withRetry.ts, filesApi.ts, dumpPrompts.ts, client.ts, errors.ts, bootstrap.ts, usage.ts, logging.ts, promptCacheBreakDetection.ts, sessionIngress.ts, errorUtils.ts, metricsOptOut.ts, firstTokenDate.ts, emptyUsage.ts (15 files).
**v2/v3 deferred**: grove.ts, referral.ts, overageCreditGrant.ts, adminRequests.ts, ultrareviewQuota.ts (5 files — account/billing features).

## File-Level Detail: services/compact/ (→ coco-compact)

All 11 files in `services/compact/` map to `coco-compact`. Existing doc coverage in crate-coco-compact.md noted.

| File | LOC | Purpose | Doc Status |
|------|-----|---------|------------|
| `compact.ts` | ~800 | Full compaction: summarization prompt, message selection, API call | Documented |
| `microCompact.ts` | ~300 | Micro-compaction: clear old tool results, preserve recent | Documented |
| `autoCompact.ts` | ~200 | Auto-compact trigger: token threshold detection | Documented |
| `sessionMemoryCompact.ts` | ~400 | Session memory: background summarization agent | Documented |
| `grouping.ts` | 64 | Groups messages at API-round boundaries (by assistant.id) | Documented (audit R1-3) |
| `postCompactCleanup.ts` | 78 | Clears 10+ caches after compaction (classifier approvals, memory, system prompt) | Documented (audit R1-3) |
| `apiMicrocompact.ts` | 154 | API-level context: clear_tool_uses, clear_thinking strategies | Documented (R6) |
| `timeBasedMCConfig.ts` | 44 | GrowthBook config: 60-min gap threshold, keep-recent=5 | Documented (audit R1-3) |
| `prompt.ts` | 374 | Compact prompt generation: proactive mode branching, cache-sharing fork paths | **NEW** |
| `compactWarningState.ts` | 18 | State store: suppress autocompact warning after successful compaction | **NEW** |
| `compactWarningHook.ts` | 16 | React hook for warning state subscription (no Rust port — UI wiring) | **NEW** (SKIP) |
| `reactiveCompact.ts` | — | Reactive compaction (feature-gated prompt_too_long recovery) | v2 (already mapped) |

## File-Level Detail: utils/telemetry/ (→ coco-otel)

Already mapped as directory (9 files → coco-otel). Detailed for implementation reference:

| File | LOC | Purpose | Notes |
|------|-----|---------|-------|
| `sessionTracing.ts` | 927 | Session tracing: root interaction spans, operation spans | L2 span hierarchy |
| `instrumentation.ts` | 825 | Main OTEL setup: protocol-specific exporters (OTLP/Prometheus) | L0-L1 |
| `perfettoTracing.ts` | 1,120 | Perfetto Chrome trace format: agent hierarchy, API request details | L5 (ant-only) |
| `betaSessionTracing.ts` | 491 | Beta session tracing: org allowlisting, system prompt/model output visibility | L5 (ant-only) |
| `pluginTelemetry.ts` | 289 | Plugin telemetry: twin-column privacy pattern, redacted names, opaque hash IDs | L3 events |
| `bigqueryExporter.ts` | 252 | BigQuery push metric exporter via OTEL metrics SDK | L5 exporter |
| `events.ts` | 75 | Event sequencing, user prompt logging with redaction control | L3 events |
| `skillLoadedEvent.ts` | 39 | Logs tengu_skill_loaded event per available skill at session startup | L3 events |
| `logger.ts` | 26 | OpenTelemetry diagnostic logger wrapping diag errors/warnings | L0 |

## cocode-rs Crates (copy, not from TS)

| cocode-rs crate | coco-rs crate | Rust dir | Strategy | Justification |
|----------------|---------------|----------|----------|---------------|
| `common/error` + `stack-trace-macro` | `coco-error` | `common/error/` | cocode | **Rust-only**: snafu + proc-macro |
| `common/otel` | `coco-otel` | `common/otel/` | HYBRID | OTel structure + TS analytics |
| `utils/*` (24 crates) | `coco-*` (renamed) | `utils/*/` | cocode | **Rust-only** infrastructure |
| `vercel-ai/*` (8 crates) | (no rename) | `vercel-ai/*/` | cocode | **No TS equiv**: multi-provider SDK |
| `exec/sandbox` | `coco-sandbox` | `exec/sandbox/` | cocode | **Rust superior**: seccomp, Seatbelt |
| `exec/process-hardening` | `coco-process-hardening` | `exec/process-hardening/` | cocode | **Rust-only**: libc FFI |
| `lsp/` | `coco-lsp` | `services/lsp/` | cocode | **Rust superior**: AI-friendly symbol queries |
| `retrieval/` | `coco-retrieval` | `retrieval/` | cocode | **No TS equiv**: BM25+vector search (optional) |

## Counts

| Version | TS dirs mapped | Rust crates |
|---------|---------------|-------------|
| v1 | 79 TS dirs/files | 25 new crates (in 64 total with 39 cocode-rs copy crates) |
| v2 | 14 TS dirs | 4 new crates: coordinator, vim, voice, assistant |
| v3 | 9 TS dirs | 2 new crates: remote (includes proxy) + ~7 TBD |
| SKIP | 5 TS dirs | 0 |
| **Total** | **107** | **~77** (64 v1 + 4 v2 + 2 v3 + ~7 TBD) |

Plan doc coverage: 21 docs cover all 31 non-copy crates (v1: 16 docs → 25 crates, v2: 4 docs → 4 crates, v3: 1 doc → 2 crates).

Note: 1884 total TS files. Directory-level mapping covers all files — individual files within a mapped directory inherit its crate assignment. `ts-utils-mapping.md` covers the 298 top-level `utils/*.ts` files individually. File-level detail sections above enumerate individual files within `services/api/` (20 files), `services/compact/` (11 files), and `utils/telemetry/` (9 files) for implementation reference.
