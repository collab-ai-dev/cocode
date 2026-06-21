# coco-hooks

Pre/post event interception with scoped priority: Command / Prompt / Http / Agent handlers, SSRF guard, async hook registry, `if` permission-rule conditions, matcher patterns (exact / pipe-separated / regex / glob), dedup + `once` tracking, HTTP URL allowlist + per-hook env-var allowlist, `expectedHookEvent` JSON cross-check.

## Key Types
- `HookDefinition` — `event` (`HookEventType`, 27 variants matching TS), `matcher`, `handler`, `priority` (asc), `scope` (Session>Local>Project>User>Builtin), `if_condition`, `once`, `is_async`, `async_rewake`, `status_message`
- `HookHandler` — `Command{command,timeout_ms,shell}` / `Prompt{prompt,model,timeout_ms}` / `Http{url,headers,timeout_ms,allowed_env_vars}` / `Agent{prompt,model,timeout_ms}`
- `FunctionHookPredicate` (trait) — `evaluate(&[Arc<Message>]) -> bool` + `name()`. Implementations are `Send + Sync + Debug` and must be pure. Used by `FunctionHook` (an in-memory hook registered at session bootstrap rather than loaded from settings) to express in-process callbacks like `StructuredOutput` Stop enforcement and Swarm teammate init. Stored on `HookRegistry.function_hooks` (separate from settings-loaded `hooks` because closures cannot `Serialize` / `Deserialize`).
- `HookEvaluationResult` — `Ok` / `Blocking{reason}` / `Cancelled` / `NonBlockingError{error}` for LLM-driven Prompt/Agent paths
- `HookLlmHandle` (trait) — async `evaluate_prompt` / `evaluate_agent` callbacks installed via `OrchestrationContext.llm_handle`; impl lives in `coco-query` to keep coco-hooks below the inference layer
- `HookExecutionResult` — `CommandOutput{exit_code,stdout,stderr}` or `PromptText(String)`
- `HookExecutionMeta`, `HookExecutionEvent` — progress display payloads
- `HooksSettings` — deserialized config wrapper
- `HookRegistry` — `register_deduped`, `find_matching[_with_if]`, `execute_hooks`, `mark_once_fired`, `register_for_agent(agent_id, hooks, is_agent)` (Stop→SubagentStop rewrite when `is_agent: true`), `clear_agent_scope`, `register_function_hook(id, event, matcher, timeout, predicate, error_message)` / `remove_function_hook(id)` / `find_matching_function_hooks(event, matcher)` / `function_hook_count()` for the in-memory `FunctionHook` overlay
- `IfConditionContext` — tool name + content for `"Bash(git *)"`-style conditions
- `PromptRequest` / `PromptResponse` / `PromptOption` — interactive hook prompts via stdout/stdin
- `OrchestrationContext` — carries: `session_id`, `cwd`, `project_dir`, `permission_mode`, `transcript_path`, `agent_id`, `agent_type`, `cancel`, `disable_all_hooks`, `allow_managed_hooks_only`, `attachment_emitter`, `sync_event_sink`, `http_url_allowlist`, `http_env_var_policy`, `async_registry`, `llm_handle`
- `SESSION_HOOK_EVENTS` — 10 session-level event names

## Key Functions
- `execute_hook()` — Command via `sh -c` + stdin piping (30 s default), Prompt/Agent text-passthrough fallback (real LLM eval lives in `run_hook_via_handle_or_fallback` once `llm_handle` is installed), HTTP defaults to **10-minute** timeout, hardcoded POST, allowlist-gated env-var interpolation, CRLF-sanitized headers, SSRF gate (private/link-local block, loopback allowed)
- `load_hooks_from_config()` — deserialize snake_case event-keyed JSON; accepts both `allowed_env_vars` and `allowedEnvVars`; `model` honored on Prompt + Agent; top-level `timeout` (sec) applied to Command/Http/Prompt/Agent when handler-level `timeout_ms` absent
- `matcher_matches()` — `None` matches all, `"*"` requires a value, simple alnum/`_`/`|`, else regex with glob fallback
- `aggregate_results_for_event()` — event-name cross-check: when `hookSpecificOutput.hookEventName` doesn't match the firing event, the nested fields are skipped with a warning instead of silently applied

## Orchestration Entry Points

All take `&HookRegistry, &OrchestrationContext, ...` and return `AggregatedHookResult` (or a richer per-event result for compaction). 27 events, 24 entry points (Compact has separate Pre/Post helpers; TaskCreated/TaskCompleted/TeammateIdle have distinct input structs but share an internal `run_event_with_input` runner). Every event has at least one wired trigger site:

| Event | Function | Trigger sites in coco-rs |
|---|---|---|
| PreToolUse | `execute_pre_tool_use` | `app/query/hook_controller.rs`, `app/query/hook_adapter.rs` |
| PostToolUse | `execute_post_tool_use` | same |
| PostToolUseFailure | `execute_post_tool_use_failure` | same |
| SessionStart | `execute_session_start` | `app/cli/session_runtime.rs`, `app/query/engine_compaction.rs` |
| UserPromptSubmit | `execute_user_prompt_submit` | `app/cli/session_runtime.rs` |
| SessionEnd | `execute_session_end` | `app/cli/session_runtime.rs` (`/clear`) |
| Stop | `execute_stop` (takes `history: &[Arc<Message>]` for `FunctionHookPredicate` dispatch) | `app/query/engine.rs` |
| StopFailure | `execute_stop_failure` | `app/query/engine_session.rs` |
| SubagentStart | `execute_subagent_start` | `coordinator/agent_handle/spawn.rs` |
| SubagentStop | `execute_subagent_stop` | same |
| PreCompact | `execute_pre_compact` | `app/query/engine_compaction.rs` |
| PostCompact | `execute_post_compact` | same |
| Setup | `execute_setup` | `app/cli/main.rs` (Maintenance), `app/cli/session_runtime.rs:fire_setup_hooks` |
| Notification | `execute_notification` | `app/cli/tui_permission_bridge.rs`, `app/cli/sdk_server/approval_bridge.rs`, `app/cli/sdk_server/sandbox_approval_bridge.rs` (`permission_prompt`); `app/cli/tui_runner.rs::FireIdleNotification` (`idle_prompt`); `app/cli/elicitation_hooks.rs::run_result_hook_and` (`elicitation_response`) |
| PermissionRequest | `execute_permission_request` | `app/query/permission_controller.rs::resolve_ask` (fires before the bridge prompt; hook decisions short-circuit Allow/Deny) |
| PermissionDenied | `execute_permission_denied` | `app/query/tool_call_preparer.rs::maybe_fire_permission_denied_hook` (auto-mode classifier denials; retry flag rewrites the deny message) |
| Elicitation | `execute_elicitation` | `app/cli/elicitation_hooks.rs::wrap_send_elicitation_with_hooks` (wraps `coco_mcp::SendElicitation` so hook decisions short-circuit the no-op dialog stub) |
| ElicitationResult | `execute_elicitation_result` | same — `run_result_hook_and` runs after the dialog/short-circuit, can override action/content |
| ConfigChange | `execute_config_change` | `app/cli/session_runtime.rs::spawn_config_change_watcher` (subscribes to `RuntimeReloader::subscribe_changes`; TUI runner installs it post-build) |
| InstructionsLoaded | `execute_instructions_loaded` | `app/query/engine_attachments.rs::drain_nested_memory_triggers` (per newly-loaded `MemoryFileSource`) |
| CwdChanged | `execute_cwd_changed` | `app/cli/session_runtime.rs::fire_cwd_changed_hooks` (drains `watch_paths` into `FileChangedHookWatcher`) |
| FileChanged | `execute_file_changed` | `app/cli/file_changed_watcher.rs::FileChangedHookWatcher` (wraps `coco_file_watch::FileWatcher` with 250 ms throttle; paths registered from `SessionStart` / `CwdChanged` hook output) |
| WorktreeCreate | `execute_worktree_create` | `coordinator/agent_handle/spawn.rs::fire_worktree_create_hook` (post-`AgentWorktreeManager::create_for`) |
| WorktreeRemove | `execute_worktree_remove` | `coordinator/agent_handle/spawn.rs::fire_worktree_remove_hook` (post-`cleanup_if_unchanged` on `Removed` outcome only — `Kept` preserves user work) |
| TaskCreated | `execute_task_created` | `core/tools/task_tools.rs::TaskCreateTool::run` (fires after persist; rolls back via `delete_task` on block) |
| TaskCompleted | `execute_task_completed` | `core/tools/task_tools.rs::TaskUpdateTool::run` (fires before persist on `newly_completed`; returns error to user on block) |
| TeammateIdle | `execute_teammate_idle` | `coordinator/runner_loop.rs` (fires before idle transition; blocking hook keeps the teammate working) |

Each entry point flows policy fields off `OrchestrationContext` (HTTP allowlist, env-var policy, async registry, LLM handle).

## Notification subtypes

`execute_notification` carries an opaque `notification_type` string. Coco-rs fires five types (`auth_success` is intentionally omitted — see "Skipped"):

| `notification_type` | Trigger | Site |
|---|---|---|
| `permission_prompt` | Tool permission dialog opens | `tui_permission_bridge.rs`, `sdk_server/approval_bridge.rs`, `sdk_server/sandbox_approval_bridge.rs` |
| `idle_prompt` | User idle past `IDLE_PROMPT_THRESHOLD` (60 s) after a turn completes | `app/tui/app.rs::maybe_fire_idle_prompt` → `UserCommand::FireIdleNotification` → `tui_runner.rs` |
| `elicitation_response` | After an MCP elicitation resolves (any action) | `elicitation_hooks.rs::run_result_hook_and` |
| `elicitation_dialog` | MCP elicitation dialog opens | **Pending** — depends on a TUI dialog UI for MCP elicitations (no equivalent today; the wrap closure short-circuits before any dialog) |
| `elicitation_complete` | After an MCP elicitation dialog closes | **Pending** — same |

## Modules
- `async_registry` — capture stdout/stderr/exit-code of `is_async` hooks for delivery via the reminder pipeline
- `inputs` — per-event input structs flatten `BaseHookInput` (now carrying `agent_id`/`agent_type`)
- `llm_handle` — `HookLlmHandle` trait + `HookEvaluationResult` for LLM-driven Prompt / Agent hooks
- `orchestration` — parallel hook execution, env vars, stdin, JSON output parsing, event-tagged aggregation
- `reminder_source` — `CombinedHookEventsSource` bridges async-registry + sync-buffer into the reminder pipeline
- `ssrf` — URL → IP resolution + private/link-local blocklist + URL-allowlist matcher
- `sync_hook_buffer` — FIFO of completed sync hook events for the per-turn reminder pipeline

## Skipped (intentional)

- `auth_success` notification — Coco-rs handles auth via `coco-cli login` / settings flows that already render their own confirmation; bolting a `Notification` hook onto them would only fire on interactive UX paths the user just initiated, which never matched the spirit of the hook (background notifiers). Leave the variant available in case a non-interactive auth path lands later.

## Pending (TUI dialog UI required)

- `elicitation_dialog` / `elicitation_complete` notification subtypes — both fire only when an MCP elicitation actually opens a dialog. Today the `SendElicitation` callsites have no UI; the hook wrapper short-circuits or auto-rejects before any dialog could open. When a TUI elicitation dialog lands, fire these from the dialog-show / dialog-close edges.

## Done in earlier waves (kept here as anchors)

- File watcher for `FileChanged` — `app/cli/file_changed_watcher.rs` over `coco_file_watch::FileWatcher` (notify), 250 ms throttle, paths registered from `SessionStart` / `CwdChanged` `hookSpecificOutput.watchPaths`.
- `${CLAUDE_PLUGIN_ROOT}` / `${CLAUDE_PLUGIN_DATA}` / `${user_config.X}` substitution — `hooks/src/lib.rs::substitute_plugin_vars`, applied in the Command branch of `execute_hook`.
- `normalizeLegacyToolName` — `coco_types::normalize_legacy_tool_name` + `legacy_tool_name_aliases_of`; `matcher_matches` runs both canonical and legacy aliases (Task→Agent, KillShell→TaskStop, AgentOutputTool/BashOutputTool→TaskOutput).
- SDK `includeHookEvents` opt-in — `QueryEngineConfig.include_hook_events` (default `false`); `engine_session.rs` only opens the `hook_event_tx` channel when set; `sdk_server/sdk_runner.rs` reads `runtime.current_engine_config()`; subagents never propagate the flag.

## Open (loader semantics)

- `hooksConfigSnapshot` capture/refresh semantics — a pinned snapshot per session enables deterministic re-fires; the Rust loader is stateless (low priority — only matters for hot-reload edge cases where a hook is removed mid-turn).
