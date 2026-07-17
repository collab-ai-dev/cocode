# coco-permissions

Permission evaluation pipeline: auto-mode / yolo classifier (2-stage XML via LLM), denial tracking, rule compilation, shell rule matching, dangerous-pattern detection.

## Key Types

| Area (module) | Key items |
|---|---|
| Evaluation (`evaluate`) | `PermissionEvaluator`, `ToolCheckResult`, `evaluate_with_tool_check` |
| Auto mode (`auto_mode*`) | `AutoModeInput`, `AutoModeState`, `AutoModeRules`, `classify_for_auto_mode`, `is_safe_tool` |
| Yolo classifier (`classifier`) | `classify_yolo_action`, `YoloClassifierResult`, `ClassifyRequest` |
| Rule compiler (`rule_compiler`) | `compile_rules`, `evaluate_rules_for_tool`, `parse_rule_string` |
| Mode transitions (`mode_transition`) | `get_next_permission_mode`, `resolve_predefined_mode`, `resolve_subagent_mode` |
| Filesystem safety (`filesystem`, `file_rules`) | `check_path_safety_for_auto_edit`, `is_dangerous_file_path`, `path_in_working_path` |
| Stores + updates (`permissions_store`, `settings_store`, `permission_updates`) | `PermissionStore`, `SettingsPermissionStore`, `apply_permission_updates` |
| Setup (`setup`) | `compute_auto_mode_capability`, `validate_permission_configuration`, `get_default_rules_for_mode` |
| Shadowed rules (`shadowed_rules`) | `detect_unreachable_rules`, `UnreachableRule`, `ShadowType` |
| Bypass / killswitch (`bypass_permissions_killswitch`) | `resolve_initial_permission_mode`, `check_bypass_killswitch_transition`, `compute_bypass_capability` |
| Explainer (`explainer`) | `generate_permission_explanation`, `build_explainer_query` |
| Misc (`denial_tracking`, `dangerous_rules`, `shell_rules`, `web_preapproved`) | `DenialTracker`, `strip_dangerous_rules` / `restore_dangerous_rules`, `ShellPermissionRule` |

**Auto-mode availability divergence**: `compute_auto_mode_capability` is
default-on, gated only by the `auto_mode.disabled` settings opt-out (no
GrowthBook `TRANSCRIPT_CLASSIFIER` / circuit breaker / `modelSupportsAutoMode`
allow-list, unlike TS). Threaded `StartupPermissionState.auto_available` →
TUI `SessionState.auto_mode_available`.

## Priority (more-specific wins)

```
session > command > cliArg > flagSettings > localSettings > projectSettings > userSettings > policySettings
```
Deny always wins immediately (step 1 of eval pipeline), regardless of priority.

## Auto-mode classifier-failure posture (fail open vs closed)

Two classifier outcomes map to human-review-or-deny in `auto_mode_decision.rs`:

- **`transcript_too_long`** (deterministic context overrun — retry can't
  help) → manual prompt when interactive, deny when headless. Iron-gate
  skipped for this case; coco-rs matches upstream.
- **`unavailable`** (transient transport/capacity outage) → **fail closed
  (deny) by default**, even interactive. Instead of a GrowthBook flag,
  coco-rs uses `auto_mode.classifier_unavailable_fail_open` (`AutoModeConfig`
  → `AutoModeRules`, default `false` = fail closed); `true` restores a manual
  interactive prompt. Headless always denies (no prompt is reachable).

Both branches deny in headless via `require_interactive_or_deny`, which keys
off the **permission-specific** `avoid_permission_prompts` (not session-level
`is_non_interactive`).

## Default `Tool::check_permissions` returns `Passthrough` (not `Allow`)

Upstream auto-allows tools without an override. coco-rs deliberately
diverges: the default is `ToolCheckResult::Passthrough`, deferring to the
rule pipeline and mode fallthrough. Tradeoff: upstream's `Allow` default
skips prompts for safe tools but silently auto-allows any gating tool that
forgets its override; coco-rs prompts for any tool without an explicit allow
rule in `Default` mode — noisier but fail-secure. In `Auto` mode the
`is_safe_tool` allowlist short-circuits before the evaluator, so safe tools
still skip the classifier.

If you add a `check_permissions` override, return:
- `Passthrough` — nothing to say; defer to rules. Safest default for unsafe tools.
- `Allow { updated_input, feedback }` — positively allows (may rewrite input).
  Skips allow/ask rules + mode fallthrough at step-1c.
- `Ask { message }` — user confirmation regardless of mode (subject to
  bypass-immune carve-outs documented in `evaluate.rs`).
- `Deny { message }` — rejects outright; allow rules cannot override.

The erased-tool blanket (`ErasedTool::check_permissions` in
`core/tool-runtime::traits`) deserializes the raw `Value` to the typed input
before delegating; on deserialize failure it fails **closed** (`Ask`), never
`Passthrough` — unreachable in production because every caller holds a
`ValidatedInput`.

## Integration

The evaluator runs from `app/query::tool_call_preparer::evaluate_with_rules`,
called when no PreToolUse hook returned a permission opinion.
`Tool::check_permissions` output fills the step-1c slot of
`PermissionEvaluator::evaluate_with_tool_check` — the central rule pipeline
sits in front of every tool call.

Settings rules reach the evaluator via
`coco_config::SettingsWithSource::sourced_permission_rules()` →
`coco_agent_host::permission_rule_loader::typed_permission_rules` →
`QueryEngineConfig.{allow,deny,ask}_rules` → per-turn
`ToolUseContext.permission_context` (`app/query::tool_context::ToolContextFactory`).

Persistence ("Always Allow"): the `app/tui` approval overlay builds
`PermissionUpdate::AddRules`; `app/cli::tui_runner` applies it via
`apply_permission_updates` (live engine config) +
`SettingsPermissionStore::persist_update` for disk destinations
(`User`/`Project`/`Local`; `Session`/`CliArg`/`Command` stay in memory).
`ToolPermissionResolution.applied_updates` carries the authorized rules
through the bridge for downstream audit/logging.
