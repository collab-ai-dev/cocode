# coco-permissions

Permission evaluation pipeline: auto-mode / yolo classifier (2-stage XML via LLM), denial tracking, rule compilation, shell rule matching, dangerous-pattern detection.

## Key Types

- **Auto mode**: `AutoModeInput`, `AutoModeDecision`, `AutoModeState`, `AutoModeRules`, `ClassifyRequest`, `YoloClassifierResult`, `classify_for_auto_mode`, `classify_auto_mode_extended`, `classify_yolo_action`, `is_safe_tool`, `can_use_tool_in_auto_mode`
- **Evaluation**: `PermissionEvaluator`, `ToolCheckFn`, `ToolCheckResult`, `get_all_rules`, `get_content_rules_for_tool`, `get_tool_wide_rule`
- **Rule compiler**: `compile_rules`, `evaluate_rules_for_tool`, `parse_rule_string`, `rule_value_to_string`, `RuleMatchResult`
- **Mode transitions**: `get_next_permission_mode`, `resolve_predefined_mode`, `resolve_subagent_mode`, `transition_context_with_auto`, `apply_auto_transition_to_app_state`
- **Filesystem safety**: `PathSafetyResult`, `check_path_safety_for_auto_edit`, `contains_path_traversal`, `get_paths_for_permission_check`, `has_dangerous_tilde`, `has_shell_expansion`, `is_dangerous_file_path`, `is_dangerous_removal_path`, `is_editable_internal_path`, `is_readable_internal_path`, `path_in_working_path`
- **Stores**: `PermissionStore`, `PermissionRulesByBehavior`, `SettingsPermissionStore`
- **Updates**: `apply_permission_update`, `apply_permission_updates`
- **Setup**: `PermissionConfigError`, `PermissionModeChoice`, `get_default_rules_for_mode`, `is_dangerous_powershell_permission`, `validate_permission_configuration`
- **Explainer**: `ExplainerParams`, `build_explainer_query`, `explainer_tool_def`, `generate_permission_explanation`
- **Shadowed rules**: `DetectUnreachableRulesOptions`, `ShadowType`, `UnreachableRule`, `detect_unreachable_rules`
- **Bypass / killswitch**: `InitialPermissionMode`, `KillswitchCheck`, `check_bypass_killswitch_transition`, `compute_bypass_capability`, `resolve_initial_permission_mode`
- **Auto-mode availability**: `compute_auto_mode_capability` — the cycle/control gate for `Auto`. Default-on; gated only by the `auto_mode.disabled` settings opt-out (no GrowthBook `TRANSCRIPT_CLASSIFIER` / circuit breaker / `modelSupportsAutoMode` allow-list, unlike TS). Threaded `StartupPermissionState.auto_available` → TUI `SessionState.auto_mode_available`.
- **Denial tracking**: `DenialTracker`
- **Dangerous rules**: `restore_dangerous_rules`, `strip_dangerous_rules`
- **Shell rules**: `ShellPermissionRule`

## Priority (more-specific wins)

```
session > command > cliArg > flagSettings > localSettings > projectSettings > userSettings > policySettings
```
Deny always wins immediately (step 1 of eval pipeline), regardless of priority.

## Auto-mode classifier-failure posture (fail open vs closed)

Two non-recoverable / transient classifier outcomes are mapped to a
human-review-or-deny decision in `auto_mode_decision.rs`:

- **`transcript_too_long`** (deterministic context overrun — retry can't
  help) → manual prompt when interactive, deny when headless. The iron-gate
  is skipped for this case; coco-rs matches.
- **`unavailable`** (transient transport/capacity outage) → **fail closed
  (deny) by default**, even in interactive mode. Rather than a GrowthBook
  feature flag, coco-rs uses the `auto_mode.classifier_unavailable_fail_open`
  setting (`AutoModeConfig` → `AutoModeRules`, default `false` = fail closed).
  Opting in (`true`) restores a manual interactive prompt; headless always
  denies regardless (no prompt is reachable).

Both branches deny in headless via `require_interactive_or_deny`, which keys
off the **permission-specific** `avoid_permission_prompts` (not session-level
`is_non_interactive`).

## Default `Tool::check_permissions` returns `Passthrough` (not `Allow`)

The upstream design auto-allows tools without an override
(`{ behavior: 'allow', updatedInput: input }` by default). coco-rs
deliberately diverges: the default is `ToolCheckResult::Passthrough`,
which defers to the rule pipeline and mode fallthrough.

Tradeoff:
- `Allow` default (the upstream design) auto-allows safe tools (ToolSearch,
  Brief, Sleep) in `Default` mode without prompting, but requires
  every gating tool (Bash, Write, Edit, NotebookEdit, …) to
  explicitly override and return `Passthrough` to opt back into
  rules. Forgetting the override silently auto-allows.
- coco-rs (`Passthrough` default) prompts for any tool without an
  explicit allow rule in `Default` mode. Slightly noisier UX, but
  fail-secure: forgetting an override prompts rather than allows.
  In `Auto` mode, `is_safe_tool` allowlist short-circuits before the
  evaluator, so safe tools still skip the classifier.

If you add a `check_permissions` override to a tool, return:
- `Passthrough` — tool has nothing to say about this input; defer to
  rules. Safest default for unsafe tools.
- `Allow { updated_input, feedback }` — tool positively allows this
  input (and may rewrite it). Skips allow / ask rules + mode
  fallthrough at the evaluator's step-1c.
- `Ask { message }` — tool requires user confirmation regardless of
  mode (subject to bypass-immune carve-outs documented in
  `evaluate.rs`).
- `Deny { message }` — tool rejects this input outright. Cannot be
  overridden by allow rules.

## Integration

The evaluator runs from
`app/query::tool_call_preparer::evaluate_with_rules`, called when no
PreToolUse hook returned a permission opinion.
`Tool::check_permissions` returns a [`coco_types::ToolCheckResult`]
that this fn captures as the step-1c slot for
`PermissionEvaluator::evaluate_with_tool_check`. This puts the central
rule pipeline in front of every tool call.

Settings rules reach the evaluator via three layers:
1. `coco_config::SettingsWithSource::sourced_permission_rules()` →
   `(allow, deny, ask)` per-source raw lists.
2. `coco_cli::permission_rule_loader::typed_permission_rules` →
   `PermissionRulesBySource` keyed by `PermissionRuleSource`.
3. `QueryEngineConfig.{allow,deny,ask}_rules` →
   `ToolUseContext.permission_context` per turn (built in
   `app/query::tool_context::ToolContextFactory`).

Persistence chain (TS `applyPermissionUpdate` →
`persistPermissionUpdates`):
- `app/tui::update::overlay::approve_all` (the dialog "Always Allow"
  action) builds a `PermissionUpdate::AddRules { destination: Session }`
  and forwards it on `UserCommand::ApprovalResponse.permission_updates`.
- `app/cli::tui_runner::ApprovalResponse` arm calls
  `coco_permissions::apply_permission_updates` (live engine_config
  mutation) and `SettingsPermissionStore::persist_update` for
  destinations that support disk persistence
  (`User`/`Project`/`Local`); `Session`/`CliArg`/`Command` stay in
  memory.
- `ToolPermissionResolution.applied_updates` carries the user's
  authorized rules through the bridge so audit/logging downstream
  sees the intent.
