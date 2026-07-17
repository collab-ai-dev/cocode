# coco-hooks

Pre/post event interception with scoped priority: Command / Prompt / Http / Agent handlers, SSRF guard, async hook registry, `if` permission-rule conditions, matcher patterns (exact / pipe/comma-separated / regex / glob), dedup + `once` tracking, HTTP URL allowlist + per-hook env-var allowlist, `expectedHookEvent` JSON cross-check.

## Key Types
- `HookDefinition` — `event` (`HookEventType`), `matcher`, `handler`, `priority` (asc), `scope` (Session>Local>Project>User>Builtin), `if_condition`, `once`, `is_async`, `async_rewake`, `status_message`
- `HookHandler` — `Command{command,timeout_ms,shell}` / `Prompt{prompt,model,timeout_ms}` / `Http{url,headers,timeout_ms,allowed_env_vars}` / `Agent{prompt,model,timeout_ms}`
- `FunctionHookPredicate` (trait) — `evaluate(&[Arc<Message>]) -> bool` + `name()`; pure, `Send + Sync + Debug`. Used by `FunctionHook` — an in-memory hook registered at session bootstrap (e.g. Swarm teammate init), stored on `HookRegistry.function_hooks` separately because closures can't `Serialize`.
- `HookEvaluationResult` — `Ok` / `Blocking{reason}` / `Cancelled` / `NonBlockingError{error}` for LLM-driven Prompt/Agent paths
- `HookLlmHandle` (trait) — async `evaluate_prompt` / `evaluate_agent` installed via `OrchestrationContext.llm_handle`; impl lives in `coco-query` to keep coco-hooks below the inference layer
- `HookExecutionResult` — `CommandOutput{exit_code,stdout,stderr}` or `PromptText(String)`; `HookExecutionMeta` / `HookExecutionEvent` — progress display payloads
- `HooksSettings` — deserialized config wrapper
- `HookRegistry` — `register_deduped`, `find_matching[_with_if]`, `execute_hooks`, `mark_once_fired`, `register_for_agent(agent_id, hooks, is_agent)` (Stop→SubagentStop rewrite when `is_agent: true`), `clear_agent_scope`, plus `register_function_hook` / `remove_function_hook` / `find_matching_function_hooks` for the in-memory overlay
- `IfConditionContext` — tool name + content for `"Bash(git *)"`-style conditions
- `PromptRequest` / `PromptResponse` / `PromptOption` — interactive hook prompts via stdout/stdin
- `OrchestrationContext` — session/agent identity, cwd/project_dir, permission_mode, transcript_path, cancel, `disable_all_hooks`, `allow_managed_hooks_only`, attachment/sync-event sinks, `http_url_allowlist`, `http_env_var_policy`, `async_registry`, `llm_handle`

## Key Functions
- `execute_hook()` — Command via `sh -c` + stdin piping (30 s default), Prompt/Agent text-passthrough fallback (real LLM eval via `llm_handle` when installed), HTTP defaults to **10-minute** timeout, hardcoded POST, allowlist-gated env-var interpolation, CRLF-sanitized headers, SSRF gate (private/link-local block, loopback allowed)
- `load_hooks_from_config()` — snake_case event-keyed JSON; accepts `allowed_env_vars` and `allowedEnvVars`; `model` honored on Prompt + Agent; top-level `timeout` (sec) applies when handler-level `timeout_ms` absent
- `matcher_matches()` — `None` matches all, `"*"` requires a value, simple alnum/`_`/`|`/`,`/space, else regex with glob fallback. Runs both canonical and legacy tool aliases via `coco_types::normalize_legacy_tool_name` (Task→Agent, KillShell→TaskStop, AgentOutputTool/BashOutputTool→TaskOutput).
- `aggregate_results_for_event()` — when `hookSpecificOutput.hookEventName` doesn't match the firing event, the nested fields are skipped with a warning instead of silently applied
- `substitute_plugin_vars` — `${CLAUDE_PLUGIN_ROOT}` / `${CLAUDE_PLUGIN_DATA}` / `${user_config.X}` substitution, applied in the Command branch of `execute_hook`

## Orchestration Entry Points

All take `&HookRegistry, &OrchestrationContext, ...` and return `AggregatedHookResult` (or a richer per-event result for compaction). Every event has at least one wired trigger site. Trigger column is crate-level (module in parens); exact file paths rot — grep the `execute_*` name.

| Event | Function | Fired from |
|---|---|---|
| PreToolUse / PostToolUse / PostToolUseFailure | `execute_pre_tool_use` etc. | `app/query` (hook_controller, hook_adapter) |
| SessionStart | `execute_session_start` | `app/agent-host` (session_runtime), `app/query` (engine_compaction) |
| UserPromptSubmit | `execute_user_prompt_submit` | `app/agent-host` (session_runtime) |
| SessionEnd | `execute_session_end` | `app/agent-host` (session_runtime; `/clear`) |
| Setup | `execute_setup` | `app/agent-host` (session_runtime) |
| Stop | `execute_stop` (takes `history` for `FunctionHookPredicate` dispatch) | `app/query` (engine_stop_hooks) |
| StopFailure | `execute_stop_failure` | `app/query` (engine_session) |
| SubagentStart / SubagentStop | `execute_subagent_start/_stop` | `coordinator` (agent_handle/spawn) |
| PreCompact / PostCompact | `execute_pre_compact` / `execute_post_compact` | `app/query` (engine_compaction) |
| Notification | `execute_notification` | `app/agent-host` (permission/sandbox bridges, elicitation_hooks); idle path from `app/tui` via `app/cli` TUI driver |
| PermissionRequest | `execute_permission_request` | `app/query` (permission_controller::resolve_ask — fires before the bridge prompt; hook decisions short-circuit Allow/Deny) |
| PermissionDenied | `execute_permission_denied` | `app/query` (tool_call_preparer::maybe_fire_permission_denied_hook — auto-mode classifier denials; retry flag rewrites the deny message) |
| Elicitation / ElicitationResult | `execute_elicitation[_result]` | `app/agent-host` (elicitation_hooks wraps `coco_mcp::SendElicitation`; result hook can override action/content) |
| ConfigChange | `execute_config_change` | `app/agent-host` (session_runtime; subscribes `RuntimeReloader::subscribe_changes`) |
| InstructionsLoaded | `execute_instructions_loaded` | `app/query` (engine_attachments::drain_nested_memory_triggers, per newly-loaded `MemoryFileSource`) |
| CwdChanged | `execute_cwd_changed` | `app/agent-host` (session_runtime; drains `watch_paths` into the file-changed watcher) |
| FileChanged | `execute_file_changed` | `app/agent-host` (file_changed_watcher over `coco_file_watch`, 250 ms throttle; paths registered from SessionStart/CwdChanged `watchPaths` output) |
| WorktreeCreate / WorktreeRemove | `execute_worktree_create/_remove` | `coordinator` (agent_handle/spawn; Remove fires only on `Removed` outcome — `Kept` preserves user work) |
| TaskCreated / TaskCompleted | `execute_task_created/_completed` | `app/query` (hook_adapter), fired from `core/tools` Task tools — Created rolls back via `delete_task` on block; Completed fires before persist and errors on block |
| TeammateIdle | `execute_teammate_idle` | `coordinator` (runner_loop; blocking hook keeps the teammate working) |

Each entry point flows policy fields off `OrchestrationContext` (HTTP allowlist, env-var policy, async registry, LLM handle).

## Notification subtypes

`execute_notification` carries an opaque `notification_type` string. Coco-rs fires:

| `notification_type` | Trigger | Site |
|---|---|---|
| `permission_prompt` | Tool permission dialog opens | `app/agent-host` (tui_permission_bridge, app_server_host permission + sandbox approval bridges) |
| `idle_prompt` | User idle past `IDLE_PROMPT_THRESHOLD` (60 s) after a turn completes | `app/tui` (`maybe_fire_idle_prompt` → `UserCommand::FireIdleNotification`) → `app/cli` TUI driver |
| `elicitation_response` | After an MCP elicitation resolves (any action) | `app/agent-host` (elicitation_hooks::run_result_hook_and) |
| `elicitation_dialog` / `elicitation_complete` | MCP elicitation dialog opens / closes | **Pending** — needs a TUI elicitation dialog; today the wrap closure short-circuits before any dialog |

## Modules
- `async_registry` — capture stdout/stderr/exit-code of `is_async` hooks for delivery via the reminder pipeline
- `function_hook` — `FunctionHook` / `FunctionHookPredicate` in-memory overlay
- `inputs` — per-event input structs flatten `BaseHookInput` (carries `agent_id`/`agent_type`)
- `llm_handle` — `HookLlmHandle` trait + `HookEvaluationResult` for LLM-driven Prompt / Agent hooks
- `orchestration` — parallel hook execution, env vars, stdin, JSON output parsing, event-tagged aggregation
- `reminder_source` — `CombinedHookEventsSource` bridges async-registry + sync-buffer into the reminder pipeline
- `ssrf` — URL → IP resolution + private/link-local blocklist + URL-allowlist matcher
- `sync_hook_buffer` — FIFO of completed sync hook events for the per-turn reminder pipeline
- `error` — crate error type

## Invariants / integration facts
- SDK `includeHookEvents` opt-in: `QueryEngineConfig.include_hook_events` (default `false`); the engine only opens the hook-event channel when set; subagents never propagate the flag.
- Legacy tool-name aliases are matched in `matcher_matches` (see Key Functions) — hooks written against old TS tool names keep firing.

## Skipped (intentional)
- `auth_success` notification — auth flows (`coco-cli login` / settings) already render their own confirmation and are user-initiated interactive paths, never matching the hook's background-notifier spirit. Variant stays available if a non-interactive auth path lands.

## Pending (TUI dialog UI required)
- `elicitation_dialog` / `elicitation_complete` — fire only when an MCP elicitation actually opens a dialog; no such UI exists yet. When a TUI elicitation dialog lands, fire these from the dialog-show / dialog-close edges.

## Open (loader semantics)
- `hooksConfigSnapshot` capture/refresh — a pinned per-session snapshot would enable deterministic re-fires; the Rust loader is stateless (low priority — only matters when a hook is removed mid-turn during hot-reload).
