# TS `utils/*.ts` → Rust Complete Mapping

All 338 top-level TS utils files mapped to their Rust destination.

**Legend:**
- **Reuse**: Maps to existing cocode-rs `utils/` crate or Rust std/crates.io
- **New util**: New Rust utils crate needed (generic infrastructure)
- **→ crate**: Application logic, belongs in a feature crate
- **Skip**: Not needed in Rust (TS-specific, React-only, or covered by std)

---

## A. Generic Infrastructure → Rust Utils (reuse or new)

### A1. Maps to existing cocode-rs `utils/` or std/crates.io

| TS file | Rust equivalent | Notes |
|---------|----------------|-------|
| `CircularBuffer.ts` | `std::collections::VecDeque` | Not needed as crate |
| `array.ts` | `itertools` / std iter | intersperse, uniq — std iterators |
| `set.ts` | `std::collections::HashSet` | difference, intersects — std methods |
| `stream.ts` | `utils/async-utils` / `tokio_stream` | AsyncIterator → tokio Stream |
| `signal.ts` | `utils/async-utils` / `tokio::sync::broadcast` | Event emitter pattern |
| `abortController.ts` | `utils/async-utils` / `CancellationToken` | Already in cocode-rs |
| `combinedAbortSignal.ts` | `CancellationToken::child_token()` | Already in tokio-util |
| `sleep.ts` | `tokio::time::sleep` + cancel | Trivial in Rust |
| `memoize.ts` | `utils/cache` | LRU memoization → `coco-cache` |
| `hash.ts` | `sha2` crate | djb2, sha256 |
| `crypto.ts` | `uuid::Uuid::new_v4()` | randomUUID |
| `uuid.ts` | `uuid` crate | validateUuid, agentId format |
| `semver.ts` | `semver` crate | gt, gte, lt, lte, eq |
| `tempfile.ts` | `tempfile` crate | Temp file paths |
| `findExecutable.ts` | `which` crate | PATH lookup |
| `which.ts` | `which` crate | whichSync/whichAsync |
| `lockfile.ts` | `fd-lock` or `fs2` crate | File locking |
| `errors.ts` | `common/error` (`coco-error`) | ClaudeError → snafu errors |
| `withResolvers.ts` | Not needed | Promise.withResolvers → Rust has no equivalent (use channels) |
| `objectGroupBy.ts` | `itertools::group_by` | Not needed as util |
| `semanticBoolean.ts` | Not needed | Trivial boolean helpers |
| `semanticNumber.ts` | Not needed | Trivial number helpers |

### A2. Fold into `utils/common` (existing crate, expand)

| TS file | What it does | Notes |
|---------|-------------|-------|
| `cwd.ts` | pwd(), getCwd() | → `find_coco_home()` in utils/common |
| `platform.ts` | OS detection (macOS/Win/Linux/WSL) | → `std::env::consts::OS` + WSL check |
| `envUtils.ts` | Config home, env truthy/falsy | Already partially mapped |
| `format.ts` | formatFileSize, formatDuration, formatCount | Generic formatting |
| `formatBriefTimestamp.ts` | Brief timestamp formatting | Fold into format |
| `process.ts` | EPIPE handlers, stdout write | Process utilities |
| `words.ts` | adj-noun slug generation | Session ID naming |
| `taggedId.ts` | Branded ID generation | Generic ID helper |
| `xdg.ts` | XDG directory resolution | Platform config paths |
| `systemDirectories.ts` | System directory discovery | Platform paths |
| `cachePaths.ts` | Cache directory resolution | → `dirs` crate + custom |
| `genericProcessUtils.ts` | Process exit handling | Process utilities |
| `gracefulShutdown.ts` | Shutdown signal handling | `tokio::signal` |
| `warningHandler.ts` | Warning/deprecation handler | Logging |

### A3. Fold into `utils/string` (existing crate, expand)

| TS file | What it does |
|---------|-------------|
| `stringUtils.ts` | escapeRegExp, capitalize, plural, truncateString |
| `intl.ts` | Grapheme/word segmentation, i18n |
| `sliceAnsi.ts` | ANSI-aware string slicing |
| `truncate.ts` | Width-aware path/string truncation |
| `sanitization.ts` | String sanitization (HTML, XML entities) |
| `displayTags.ts` | XML-like display tag formatting |

### A4. Fold into `utils/git` (existing crate, expand)

| TS file | What it does |
|---------|-------------|
| `git.ts` (926 LOC) | Git ops: root, branch, remotes, stats, worktrees |
| `gitDiff.ts` (532 LOC) | Git diff parsing |
| `gitSettings.ts` | Git instruction inclusion setting |
| `detectRepository.ts` | Repo type detection |
| `githubRepoPathMapping.ts` | GitHub repo path mapping |

### A5. Fold into `utils/image` (existing crate, expand)

| TS file | What it does |
|---------|-------------|
| `pdf.ts` (300 LOC) | PDF rendering/metadata |
| `pdfUtils.ts` | PDF utilities |
| `imageResizer.ts` (880 LOC) | Image compression with token limits |
| `imageStore.ts` | Image storage management |
| `imageValidation.ts` | Image format/size validation |
| `imagePaste.ts` | Clipboard image handling |
| `screenshotClipboard.ts` | Screenshot capture |

### A6. Fold into `exec/shell` (shell execution layer)

| TS file | What it does |
|---------|-------------|
| `Shell.ts` (474 LOC) | Shell command execution, binary resolution |
| `ShellCommand.ts` (465 LOC) | ExecResult, spawn/kill wrapping |
| `shellConfig.ts` | Shell config detection (.bashrc, .zshrc) |
| `glob.ts` | Glob pattern execution |
| `ripgrep.ts` (679 LOC) | Ripgrep wrapper (system/builtin detection) |
| `execFileNoThrow.ts` | execFile without throwing |
| `execFileNoThrowPortable.ts` | Portable variant |
| `execSyncWrapper.ts` | Sync exec wrapper |
| `subprocessEnv.ts` | Subprocess environment setup |
| `promptShellExecution.ts` | Interactive shell prompt execution |

### A7. New `utils/frontmatter` crate

| TS file | What it does |
|---------|-------------|
| `frontmatterParser.ts` (370 LOC) | YAML frontmatter extraction from markdown |
| `yaml.ts` | YAML parsing |
| `json.ts` (277 LOC) | JSON/JSONC parsing with caching |
| `jsonRead.ts` | BOM stripping |
| `xml.ts` | XML parsing |
| `markdown.ts` (381 LOC) | Markdown processing |
| `zodToJsonSchema.ts` | Zod → JSON Schema (→ serde → JSON Schema in Rust) |

### A8. New `utils/cursor` crate (TUI input)

| TS file | What it does |
|---------|-------------|
| `Cursor.ts` (1530 LOC) | Kill ring, yank/kill ops, text editing state machine |

### A9. Fold into `utils/common` (debug/diagnostics)

| TS file | What it does |
|---------|-------------|
| `debug.ts` (268 LOC) | Debug logging with file output |
| `debugFilter.ts` | Debug filter parsing |
| `slowOperations.ts` (286 LOC) | Slow operation tracking |
| `diagLogs.ts` | Diagnostic log management |

### A10. Fold into `utils/file-encoding` or new `utils/fs`

| TS file | What it does |
|---------|-------------|
| `file.ts` (584 LOC) | writeFileSync, pathExists, safeResolvePath |
| `fsOperations.ts` (770 LOC) | FsOperations interface (portable fs abstraction) |
| `fileRead.ts` | Encoding detection, line ending detection |
| `fileReadCache.ts` | File read caching |
| `readFileInRange.ts` | Read file with offset/limit |

### A11. Fold into `utils/async-utils` (existing crate)

| TS file | What it does |
|---------|-------------|
| `timeouts.ts` | Timeout helpers |
| `sequential.ts` | Sequential execution with abort |
| `queueProcessor.ts` | Queue-based async processor |
| `mailbox.ts` | Message queue pattern |
| `QueryGuard.ts` | State machine for query lifecycle |
| `bufferedWriter.ts` | Buffered I/O writer |
| `idleTimeout.ts` | Idle timeout tracking |

### A12. Rendering utils → `app/tui`

| TS file | What it does |
|---------|-------------|
| `treeify.ts` (170 LOC) | Tree structure rendering |
| `ansiToPng.ts` (334 LOC) | ANSI → PNG |
| `ansiToSvg.ts` (272 LOC) | ANSI → SVG |
| `asciicast.ts` (239 LOC) | Asciicast recording |
| `terminal.ts` | Terminal text wrapping |
| `hyperlink.ts` | Terminal hyperlink (OSC 8) |
| `cliHighlight.ts` | CLI syntax highlighting |
| `heatmap.ts` | Heatmap rendering |
| `logoV2Utils.ts` | Logo rendering |
| `highlightMatch.tsx` | Search match highlighting |

---

## B. Application Logic → Feature Crates

### B1. → `coco-config`

| TS file | What it does |
|---------|-------------|
| `config.ts` (1817 LOC) | GlobalConfig, ProjectConfig, load/save |
| `configConstants.ts` | Config key constants |
| `env.ts` (347 LOC) | Environment variable resolution |
| `envDynamic.ts` | Dynamic env var injection |
| `envValidation.ts` | Env var validation |
| `managedEnv.ts` | Managed environment (enterprise) |
| `managedEnvConstants.ts` | Managed env var lists |
| `markdownConfigLoader.ts` (600 LOC) | Load config from markdown files (.claude/rules/) |
| `effort.ts` (329 LOC) | EffortLevel support checks |
| `fastMode.ts` | Fast mode state |
| `thinking.ts` | Thinking/reasoning config |
| `betas.ts` (434 LOC) | Beta headers per provider |
| `privacyLevel.ts` | Privacy level settings |
| `bundledMode.ts` | Bundled/bare mode detection |
| `cliArgs.ts` | CLI argument parsing |

### B2. → `coco-inference`

| TS file | What it does |
|---------|-------------|
| `api.ts` (718 LOC) | Tool schema conversion, CacheScope, system prompt blocks |
| `apiPreconnect.ts` | HTTP preconnect for latency |
| `auth.ts` (2002 LOC) | Full auth: OAuth, Bedrock, Vertex, API key |
| `authFileDescriptor.ts` | Auth via file descriptor |
| `authPortable.ts` | Portable auth wrapper |
| `aws.ts` | AWS credential management |
| `awsAuthStatusManager.ts` | AWS auth state tracking |
| `billing.ts` | Billing/subscription checks |
| `caCerts.ts` | CA certificate handling |
| `caCertsConfig.ts` | CA cert configuration |
| `mtls.ts` | Mutual TLS support |
| `proxy.ts` | HTTP proxy configuration |
| `http.ts` | HTTP client utilities |
| `modelCost.ts` (231 LOC) | Per-model pricing |
| `tokens.ts` (261 LOC) | Token extraction from API responses |
| `tokenBudget.ts` | Token budget calculations |
| `advisor.ts` | Advisor tool types |
| `userAgent.ts` | User-Agent header construction |
| `user.ts` | User type detection |
| `extraUsage.ts` | Extra usage tracking |

### B3. → `coco-context`

| TS file | What it does |
|---------|-------------|
| `context.ts` (221 LOC) | Context window management |
| `attachments.ts` (3997 LOC) | Attachment system |
| `claudemd.ts` (1479 LOC) | CLAUDE.md discovery |
| `systemPrompt.ts` | System prompt assembly |
| `systemPromptType.ts` | System prompt type definitions |
| `fileStateCache.ts` (1479 LOC) | LRU file read cache |
| `fileHistory.ts` (200 LOC) | File edit tracking per turn |
| `toolResultStorage.ts` (1040 LOC) | **MOVED** — see B-N below. Originally listed here as `ContentReplacementState`; this routing was incomplete (only Level 2; Level 1 persistence pipeline missing) and led to multi-round review miss. Re-routed to `coco-tool-runtime` (storage + enforcement) + `coco-query` (wiring) + `coco-session` (records). See [`tool-result-budget-plan.md`](tool-result-budget-plan.md). |
| `analyzeContext.ts` (1382 LOC) | Context analysis |
| `contextAnalysis.ts` | Context size analysis |
| `contextSuggestions.ts` | Context suggestions |
| `queryContext.ts` | Query context building |
| `readEditContext.ts` | Read/edit context for prompts |
| `pasteStore.ts` | Paste content cache |
| `contentArray.ts` | Content array manipulation |
| `memoryFileDetection.ts` | Memory file detection |

### B4. → `coco-messages`

| TS file | What it does |
|---------|-------------|
| `messages.ts` (5512 LOC) | 114 message functions |
| `messagePredicates.ts` | Message type predicates |
| `messageQueueManager.ts` | Message queue management |
| `collapseReadSearch.ts` (1109 LOC) | Collapse read/search tool results |
| `collapseBackgroundBashNotifications.ts` | Collapse bash notifications |
| `collapseHookSummaries.ts` | Collapse hook summaries |
| `collapseTeammateShutdowns.ts` | Collapse teammate shutdown messages |
| `groupToolUses.ts` | Group tool use messages |

### B5. → `coco-tools`

| TS file | What it does |
|---------|-------------|
| `tasks.ts` (862 LOC) | Task management |
| `toolErrors.ts` | Tool error handling |
| `toolPool.ts` | Tool pooling |
| `toolSchemaCache.ts` | Tool schema caching |
| `toolSearch.ts` (756 LOC) | Deferred tool search |
| `transcriptSearch.ts` | Transcript search |
| `embeddedTools.ts` | Embedded tool handling |
| `generatedFiles.ts` | Generated file tracking |
| `diff.ts` | Diff utilities for file tools |
| `notebook.ts` | Notebook handling |
| `worktree.ts` (1519 LOC) | Git worktree management |
| `worktreeModeEnabled.ts` | Worktree feature flag |
| `getWorktreePaths.ts` | Worktree path resolution |
| `getWorktreePathsPortable.ts` | Portable variant |
| `editor.ts` | External editor launching |
| `ghPrStatus.ts` | GitHub PR status |

### B6. → `coco-permissions`

| TS file | What it does |
|---------|-------------|
| `hooks.ts` (5022 LOC) | Hook execution pipeline |
| `classifierApprovals.ts` | Classifier approval tracking |
| `classifierApprovalsHook.ts` | Hook for classifier |
| `autoModeDenials.ts` | Auto-mode denial tracking |

### B7. → `coco-session`

| TS file | What it does |
|---------|-------------|
| `sessionStorage.ts` (5105 LOC) | Session transcript persistence |
| `sessionStoragePortable.ts` | Portable variant |
| `sessionRestore.ts` (551 LOC) | Session resume logic |
| `sessionStart.ts` | Session initialization |
| `sessionState.ts` | Session state management |
| `sessionActivity.ts` | Activity tracking |
| `sessionEnvironment.ts` | Session env setup |
| `sessionIngressAuth.ts` | Session auth |
| `sessionTitle.ts` | Session title generation |
| `sessionUrl.ts` | Session URL management |
| `sessionEnvVars.ts` | Session env vars |
| `sessionFileAccessHooks.ts` | File access hooks |
| `conversationRecovery.ts` | Conversation recovery |
| `crossProjectResume.ts` | Cross-project resume |
| `concurrentSessions.ts` | Concurrent session detection |
| `listSessionsImpl.ts` | Session listing |
| `cleanup.ts` (602 LOC) | Session cleanup |
| `cleanupRegistry.ts` | Cleanup handler registry |
| `backgroundHousekeeping.ts` | Background maintenance |

### B8. → `coco-tui`

| TS file | What it does |
|---------|-------------|
| `theme.ts` (639 LOC) | Theme management |
| `systemTheme.ts` | System dark/light detection |
| `fullscreen.ts` | Fullscreen mode |
| `textHighlighting.ts` | Text highlighting |
| `horizontalScroll.ts` | Horizontal scroll |
| `renderOptions.ts` | Render options |
| `promptEditor.ts` | Prompt editor |
| `keyboardShortcuts.ts` | Keyboard shortcuts |
| `modifiers.ts` | Key modifiers |
| `fpsTracker.ts` | FPS tracking |
| `ink.ts` | Ink renderer wrapper |
| `earlyInput.ts` | Early input handling |
| `status.tsx` | Status display |
| `statusNoticeDefinitions.tsx` | Status notices |
| `statusNoticeHelpers.ts` | Status helpers |
| `terminalPanel.ts` | Terminal panel |
| `staticRender.tsx` | Static rendering |
| `exportRenderer.tsx` | Export to image/SVG |
| `windowsPaths.ts` | Windows path display |
| `completionCache.ts` | Autocomplete cache |
| `handlePromptSubmit.ts` | Prompt submission |
| `slashCommandParsing.ts` | Slash command parsing |
| `exampleCommands.ts` | Example command display |

### B9. → `coco-cli`

| TS file | What it does |
|---------|-------------|
| `releaseNotes.ts` (360 LOC) | Release notes |
| `autoUpdater.ts` (561 LOC) | Auto-update |
| `localInstaller.ts` | Local npm install |
| `binaryCheck.ts` | Binary existence check |
| `doctorDiagnostic.ts` (625 LOC) | Diagnostics |
| `doctorContextWarnings.ts` | Context warnings |
| `startupProfiler.ts` | Startup profiling |
| `headlessProfiler.ts` | Headless profiling |
| `heapDumpService.ts` | Heap dump for debugging |
| `preflightChecks.tsx` | Preflight validation |

### B10. → `coco-query`

| TS file | What it does |
|---------|-------------|
| `queryHelpers.ts` (552 LOC) | Query helper functions |
| `queryProfiler.ts` (301 LOC) | Query performance profiling |
| `sideQuery.ts` | Side query execution |
| `sideQuestion.ts` | Side question handling |
| `argumentSubstitution.ts` | Argument substitution in prompts |

### B11. → `plugins/`

| TS file | What it does |
|---------|-------------|
| `claudeCodeHints.ts` | Plugin hints |

### B12. → `coco-tools` (AgentTool submodule)

| TS file | What it does |
|---------|-------------|
| `forkedAgent.ts` (689 LOC) | Forked agent execution, cache-safe params, subagent context |
| `agentContext.ts` | Agent context building (tool pool, system prompt assembly) |
| `agentId.ts` | Agent ID generation and validation |
| `standaloneAgent.ts` | Standalone agent mode (headless agent execution) |

### B12.5. → `coco-tool-runtime`

| TS file | What it does |
|---------|-------------|
| `toolResultStorage.ts` (1040 LOC) — Level 1 + Level 2 storage primitives | Per-tool persistence (`processToolResultBlock`, `persistToolResult`, `<persisted-output>` wrapper, 2KB preview) + per-message budget state (`ContentReplacementState`, `enforceToolResultBudget`). Query-loop wiring lives in `coco-query`; transcript records live in `coco-session`. See [`tool-result-budget-plan.md`](tool-result-budget-plan.md). |
| `constants/toolLimits.ts` | Constants module: `DEFAULT_MAX_RESULT_SIZE_CHARS=50_000`, `PREVIEW_SIZE_BYTES=2_000`, `MAX_TOOL_RESULTS_PER_MESSAGE_CHARS=200_000`, `MAX_TOOL_RESULT_BYTES=400_000`, `TOOL_RESULTS_SUBDIR='tool-results'`, `PERSISTED_OUTPUT_TAG`. |

### B13. → `coco-mcp`

| TS file | What it does |
|---------|-------------|
| `mcpInstructionsDelta.ts` | MCP instruction changes |
| `mcpOutputStorage.ts` | **CO-OWNED** with `coco-tool-runtime` — parallel pipeline to `toolResultStorage.ts` for MCP server outputs; depends on the same `<persisted-output>` + storage primitives. Phase 3 of [`tool-result-budget-plan.md`](tool-result-budget-plan.md). |
| `mcpValidation.ts` | MCP config validation |
| `mcpWebSocketTransport.ts` | MCP WebSocket transport |

### B14. → `coco-tasks`

| TS file | What it does |
|---------|-------------|
| `cronTasks.ts` (458 LOC) | Cron task management |
| `cronScheduler.ts` (565 LOC) | Cron scheduling |
| `cronTasksLock.ts` | Cron task locking |
| `cron.ts` (308 LOC) | Cron expression parsing |
| `cronJitterConfig.ts` | Cron jitter config |
| `plans.ts` (397 LOC) | Plan file management |
| `planModeV2.ts` | Plan mode v2 state |

### B15. → `coco-otel`

HYBRID 策略: 复用 cocode-rs L0-L1 (export 管道 + 7 基础事件), 从 TS 新增 L2-L5。详见 `crate-coco-otel.md`。

**utils/ 文件:**

| TS file | What it does | 目标层级 |
|---------|-------------|---------|
| `stats.ts` (1061 LOC) | Statistics collection | L4 business_metrics.rs |
| `statsCache.ts` | Stats caching | L4 business_metrics.rs |
| `telemetryAttributes.ts` | Telemetry attributes | L1 (OtelEventMetadata 已有) |
| `log.ts` (362 LOC) | Logging | L0 (tracing 已覆盖) |
| `errorLogSink.ts` | Error log sink | L3 events/ |
| `unaryLogging.ts` | Unary logging | L3 events/ |
| `sinks.ts` | Data sinks | L3 events/ |
| `fileOperationAnalytics.ts` | File op analytics | L3 events/ |

**telemetry/ 子目录 (9 files, ~4K LOC):**

| TS file | What it does | 目标层级 |
|---------|-------------|---------|
| `telemetry/sessionTracing.ts` | Hierarchical span management (interaction→tool→hook) | L2 spans/span_manager.rs |
| `telemetry/events.ts` | OTel event logging API | L3 events/ |
| `telemetry/instrumentation.ts` | Telemetry initialization + metric creation | L0 otel_provider.rs + L4 |
| `telemetry/betaSessionTracing.ts` | Beta content tracing (60KB truncation, Honeycomb) | L5 beta_tracing.rs |
| `telemetry/perfettoTracing.ts` | Chrome Trace Event format output | L5 exporters/perfetto.rs |
| `telemetry/bigqueryExporter.ts` | BigQuery custom metrics exporter | L5 exporters/bigquery.rs |
| `telemetry/logger.ts` | OTel diagnostic logger | L0 (已有) |

**services/analytics/ (8 files, ~4K LOC) — 从 coco-inference 移至 coco-otel:**

| TS file | What it does | 目标层级 |
|---------|-------------|---------|
| `analytics/index.ts` | Main event logging API (logEvent/logEventAsync) | L3 events/ |
| `analytics/firstPartyEventLogger.ts` | 1P event logger (retry, disk persistence) | L5 exporters/first_party.rs |
| `analytics/firstPartyEventLoggingExporter.ts` | 1P event OTLP exporter | L5 exporters/first_party.rs |
| `analytics/sink.ts` | Analytics sink routing | L3 events/ |
| `analytics/metadata.ts` | Event metadata formatting | L1 (OtelEventMetadata 已有) |
| `analytics/datadog.ts` | Datadog integration | L6 暂不实现 |
| `analytics/sinkKillswitch.ts` | Telemetry killswitch | L6 暂不实现 |
| `analytics/config.ts` | Analytics config | L0 config.rs |
| `analytics/growthbook.ts` | GrowthBook feature gating | L6 暂不实现 |

### B16. → `coco-bridge`

| TS file | What it does |
|---------|-------------|
| `ide.ts` (1494 LOC) | IDE detection and integration |
| `idePathConversion.ts` | IDE path conversion |
| `claudeDesktop.ts` | Claude Desktop integration |
| `jetbrains.ts` | JetBrains integration |
| `iTermBackup.ts` | iTerm backup |
| `appleTerminalBackup.ts` | Apple Terminal backup |
| `desktopDeepLink.ts` | Desktop deep linking |
| `browser.ts` | Browser launching |

### B17. → v2/v3 deferred

| TS file | What it does | Version |
|---------|-------------|---------|
| `agentSwarmsEnabled.ts` | Swarm feature flag | v2 |
| `agenticSessionSearch.ts` | Agentic search | v2 |
| `teammate.ts` | Teammate helpers | v2 |
| `teammateContext.ts` | Teammate context | v2 |
| `teammateMailbox.ts` | Teammate mailbox | v2 |
| `directMemberMessage.ts` | Direct member messaging | v2 |
| `inProcessTeammateHelpers.ts` | In-process teammate | v2 |
| `teamDiscovery.ts` | Team discovery | v2 |
| `teamMemoryOps.ts` | Team memory operations | v2 |
| `tmuxSocket.ts` | Tmux socket (tungsten) | v2 |
| `undercover.ts` | Internal testing | Skip |
| `teleport.tsx` | CCR teleport | v3 |
| `streamJsonStdoutGuard.ts` | SDK stream guard | v1 (coco-cli) |
| `streamlinedTransform.ts` | Stream transform | v1 (coco-inference) |
| `sdkEventQueue.ts` | SDK event queue | v1 (coco-cli) |
| `controlMessageCompat.ts` | Message compat | v1 (coco-messages) |
| `workloadContext.ts` | Workload context | v1 (coco-query) |
| `sinks.ts` | Data sinks | v1 (coco-otel) |
| `fingerprint.ts` | Device fingerprint | v1 (coco-cli) |
| `lazySchema.ts` | Lazy Zod schema | Not needed (serde handles this) |
| `promptCategory.ts` | Prompt categorization | v1 (coco-query) |
| `userPromptKeywords.ts` | Prompt keyword detection | v1 (coco-query) |
| `commitAttribution.ts` (961 LOC) | Commit attribution | v1 (coco-tools) |
| `attribution.ts` | Attribution management | v1 (coco-tools) |
| `fileOperationAnalytics.ts` | File op analytics | v1 (coco-otel) |
| `activityManager.ts` | Activity tracking | v1 (coco-session) |
| `generators.ts` | Generator utilities | v1 (coco-tools) |
| `immediateCommand.ts` | Immediate command execution | v1 (coco-commands) |
| `commandLifecycle.ts` | Command lifecycle hooks | v1 (coco-commands) |
| `autoRunIssue.tsx` | Auto-run issue | Skip (ant-only) |
| `codeIndexing.ts` | Code indexing | v1 (coco-tools) |

---

## Summary

| Destination | File count | LOC |
|------------|-----------|-----|
| Existing `utils/` crates (expand) | ~60 | ~6K |
| New `utils/frontmatter` | 7 | ~1.1K |
| New `utils/cursor` | 1 | 1.5K |
| `exec/shell` | 10 | ~3K |
| `app/tui` | ~23 | ~3K |
| `coco-config` | ~15 | ~6K |
| `coco-inference` | ~20 | ~6K |
| `coco-context` | ~16 | ~10K |
| `coco-messages` | ~8 | ~7K |
| `coco-tools` | ~16 | ~6K |
| `coco-session` | ~19 | ~8K |
| `coco-permissions` | ~4 | ~5K |
| `coco-query` | ~5 | ~1.5K |
| `coco-cli` | ~10 | ~2K |
| `coco-tui` | ~23 | ~3K |
| `coco-bridge` | ~8 | ~2K |
| `coco-otel` | ~25 | ~8K (复用 cocode-rs ~1.2K + 新增 TS L2-L5 ~6.8K) |
| `coco-tasks` | ~7 | ~2K |
| `coco-mcp` | ~4 | ~0.5K |
| `memory/` | ~4 | ~1K |
| `plugins/` | ~1 | tiny |
| v2/v3 deferred | ~10 | ~2K |
| Skip / Not needed | ~5 | tiny |
| **Total** | **~338** | **~80K** |

All 338 top-level `utils/*.ts` files are now accounted for.
