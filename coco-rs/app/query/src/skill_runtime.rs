//! `QuerySkillRuntime` — real implementation of
//! [`coco_tool_runtime::SkillHandle`] backed by [`coco_skills::SkillManager`].
//!
//! Phase 7-β completion: prior slice scaffolded the `SkillHandle`
//! trait with a `NoOpSkillHandle` that returned `Unavailable`; this
//! module lands the real implementation.
//!
//! ## Resolution order
//!
//! 1. Normalize name (strip leading `/`).
//! 2. Lookup in `SkillManager` (canonical name + aliases).
//! 3. Reject disabled skills (`SkillDefinition.disabled`).
//! 4. Reject model-hidden skills when `SkillTool` invokes them
//!    (`SkillDefinition.disable_model_invocation`).
//! 5. Branch on `SkillContext::{Inline, Fork}`:
//!    - Inline: expand arguments via `skill_advanced::expand_skill_prompt`
//!      and return `SkillInvocationResult::Inline { summary, new_messages }`.
//!    - Fork: route the expanded prompt through the installed
//!      `AgentQueryEngine` as a sub-agent query, return
//!      `SkillInvocationResult::Forked { agent_id, output }`.
//!
//! ## Parent tool-use id tagging
//!
//! Per plan I5, inline-expansion messages are tagged with
//! `parent_tool_use_id = <the SkillTool call id>` so they group
//! with the parent tool_result in transcripts. The runtime doesn't
//! know the SkillTool's `tool_use_id` because `SkillHandle::invoke_skill`
//! doesn't surface it — `SkillTool` downstream is responsible for
//! tagging before pushing to history. This module returns
//! untagged `UserMessage` values; the caller tags.

use std::sync::Arc;

use async_trait::async_trait;
use coco_skills::SkillContext;
use coco_skills::SkillManager;
use coco_skills::effective_skill_state;
use coco_tool_runtime::AgentQueryConfig;
use coco_tool_runtime::AgentQueryEngineRef;
use coco_tool_runtime::SkillGateContext;
use coco_tool_runtime::SkillHandle;
use coco_tool_runtime::SkillInvocationError;
use coco_tool_runtime::SkillInvocationResult;
use coco_tool_runtime::SubagentInheritance;
use coco_types::PermissionBehavior;
use coco_types::PermissionRule;
use coco_types::PermissionRuleSource;
use coco_types::PermissionUpdate;
use coco_types::PermissionUpdateDestination;
use coco_types::SkillOverrideState;

/// Real skill-runtime implementation.
///
/// Holds an `Arc<SkillManager>` directly — `SkillManager` carries its
/// own internal `RwLock` over the catalog, so the handle does not need
/// to add another layer of locking. Hot-reload from the file watcher
/// mutates the same shared `SkillManager` via `&self` register/clear
/// methods. Fork routing goes through the same
/// [`AgentQueryEngineRef`] that `SwarmAgentHandle::spawn_subagent`
/// uses, keeping one subagent execution path.
/// Late-bound, per-turn-refreshed Bash handle shared with the command
/// registry. The cell is set on every main-session engine build
/// (`SessionRuntime::build_engine`) so in-prompt `` !`cmd` `` markers run
/// through the real permission-checked Bash tool. `None` (tests /
/// pre-bootstrap) ⇒ shell markers are left untouched.
type SkillBashHandleCell =
    Arc<std::sync::RwLock<Option<Arc<dyn coco_skills::shell_exec::BashToolHandle>>>>;

pub struct QuerySkillRuntime {
    manager: Arc<SkillManager>,
    /// Optional agent query engine for fork-mode skills. `None`
    /// returns `SkillInvocationError::Forked` for any fork
    /// invocation — fine for sessions without a subagent runtime,
    /// loud-failing instead of silently ignoring.
    agent_engine: Option<AgentQueryEngineRef>,
    /// Current session id, substituted for `${CLAUDE_SESSION_ID}` in skill
    /// prompts. Installed at bootstrap via [`Self::with_session_id`]; `None`
    /// leaves the token unexpanded (older behaviour).
    session_id: Option<String>,
    /// Shared Bash handle cell for in-prompt shell expansion. `None`
    /// leaves `` !`cmd` `` markers untouched (matches the prior no-shell
    /// behaviour of the inline path).
    bash_handle: Option<SkillBashHandleCell>,
}

impl QuerySkillRuntime {
    pub fn new(manager: Arc<SkillManager>) -> Self {
        Self {
            manager,
            agent_engine: None,
            session_id: None,
            bash_handle: None,
        }
    }

    /// Install the shared Bash handle cell used to expand in-prompt
    /// `` !`cmd` `` / ```` ```! ```` markers (permission-checked). Shared with
    /// the command registry so the model-invoked path and the user-typed
    /// slash path run identical, permission-checked shell. Absent ⇒ markers
    /// are left untouched.
    pub fn with_bash_handle_cell(mut self, cell: SkillBashHandleCell) -> Self {
        self.bash_handle = Some(cell);
        self
    }

    /// Snapshot the currently-installed Bash handle, if any. Clones the
    /// `Arc` out under the lock so no guard is held across an await.
    fn snapshot_bash_handle(&self) -> Option<Arc<dyn coco_skills::shell_exec::BashToolHandle>> {
        self.bash_handle
            .as_ref()
            .and_then(|cell| cell.read().ok().and_then(|guard| guard.clone()))
    }

    /// Install the agent-query engine used for fork-mode skills.
    /// Without this, fork invocations fail with `Forked { reason }`.
    pub fn with_agent_engine(mut self, engine: AgentQueryEngineRef) -> Self {
        self.agent_engine = Some(engine);
        self
    }

    /// Install the session id used to expand `${CLAUDE_SESSION_ID}`.
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }
}

#[async_trait]
impl SkillHandle for QuerySkillRuntime {
    async fn invoke_skill(
        &self,
        name: &str,
        args: &str,
        inherit: SubagentInheritance,
        gate: SkillGateContext,
    ) -> Result<SkillInvocationResult, SkillInvocationError> {
        let name = coco_tools::tools::skill_advanced::normalize_skill_name(name);
        tracing::info!(skill_name = %name, args_len = args.len(), "skill invoke");
        let skill = self.manager.get(name).ok_or_else(|| {
            tracing::warn!(skill_name = %name, "skill not found");
            SkillInvocationError::NotFound {
                name: name.to_string(),
            }
        })?;

        if skill.disabled {
            tracing::warn!(skill_name = %skill.name, "skill disabled");
            return Err(SkillInvocationError::Disabled {
                name: skill.name.clone(),
            });
        }
        // Author lock: `disable-model-invocation: true` requires the
        // user to have typed `/<name>` in the current turn. The rejection
        // only fires when `user_invoked_via_slash` returned false. The
        // previous behavior unconditionally rejected — that was a bug.
        if skill.disable_model_invocation && !user_invoked_via_slash(&skill, &gate) {
            tracing::warn!(
                skill_name = %skill.name,
                "skill hidden from model (author lock); no user-typed slash this turn"
            );
            return Err(SkillInvocationError::HiddenFromModel {
                name: skill.name.clone(),
            });
        }
        // `skill_overrides` gate. Resolves to `On` for every skill when
        // tiers are empty (the default) — so this short-circuits to no-op
        // until users configure `skill_overrides` in their settings.json.
        let effective = effective_skill_state(&skill, &gate.overrides);
        match effective {
            SkillOverrideState::Off => {
                tracing::warn!(
                    skill_name = %skill.name,
                    "skill rejected: skill_overrides=off"
                );
                return Err(SkillInvocationError::OverrideOff {
                    name: skill.name.clone(),
                });
            }
            SkillOverrideState::UserInvocableOnly if !user_invoked_via_slash(&skill, &gate) => {
                tracing::warn!(
                    skill_name = %skill.name,
                    "skill rejected: user-invocable-only without user-typed slash"
                );
                return Err(SkillInvocationError::OverrideUserOnlyNoTrigger {
                    name: skill.name.clone(),
                });
            }
            SkillOverrideState::On
            | SkillOverrideState::NameOnly
            | SkillOverrideState::UserInvocableOnly => {
                // Pass: `on`/`name-only` always allow invocation;
                // `user-invocable-only` was permitted by the
                // user-typed-slash guard above.
            }
        }
        tracing::debug!(
            skill_name = %skill.name,
            context = ?skill.context,
            "skill resolved, expanding prompt"
        );

        // Expand argument substitutions. The expander runs before either
        // inline injection or fork spawn. Beyond `$ARGS`/positionals, the
        // skill's source directory and session id MUST be substituted so
        // `${CLAUDE_SKILL_DIR}` / `${CLAUDE_SESSION_ID}` resolve (and the
        // "Base directory for this skill:" header is prepended) — otherwise
        // the literal tokens reach the model and skill-relative file
        // resolution breaks.
        let skill_dir = skill
            .skill_root
            .as_deref()
            .and_then(std::path::Path::to_str);
        let expanded_prompt = coco_tools::tools::skill_advanced::expand_skill_prompt(
            &skill.prompt,
            &coco_tools::tools::skill_advanced::ExpandOptions {
                args,
                argument_names: &[],
                skill_dir,
                session_id: self.session_id.as_deref(),
                base_dir: skill_dir,
                plugin_root: None,
                plugin_data_dir: None,
                user_config: None,
            },
        );

        // In-prompt shell expansion (`` !`cmd` `` / ```` ```! ```` blocks),
        // permission-checked through the parent's Bash handle. Runs for both
        // inline and fork before the branch below, mirroring TS
        // `getPromptForCommand` (both paths render shell identically). An
        // `Err` (denied / failed) aborts the whole invocation. MCP-sourced
        // skills never run in-prompt shell — remote untrusted code.
        let expanded_prompt = if matches!(skill.source, coco_skills::SkillSource::Mcp { .. }) {
            expanded_prompt
        } else if let Some(bash) = self.snapshot_bash_handle() {
            let allowed = skill.allowed_tools.clone().unwrap_or_default();
            coco_skills::shell_exec::execute_shell_in_prompt_with_tool(
                &expanded_prompt,
                &*bash,
                &allowed,
            )
            .await
            .map_err(|reason| SkillInvocationError::Expansion {
                name: skill.name.clone(),
                reason,
            })?
        } else {
            expanded_prompt
        };

        // Skill frontmatter `allowed-tools` becomes Command-source
        // auto-allow rules. Inline path threads these through
        // `SkillInvocationResult::Inline.permission_updates` →
        // `ToolResult.permission_updates` → executor's
        // `PermissionRuleHandle` → session config. Fork path inlines
        // them on `AgentQueryConfig.extra_permission_rules` so the
        // subagent's first turn already sees them. Both write into
        // the `Command` permission slot (`alwaysAllowRules.command`).
        let allow_rules = skill
            .allowed_tools
            .as_deref()
            .map(build_command_allow_rules)
            .unwrap_or_default();

        let result = match skill.context {
            SkillContext::Inline => {
                // Inline: surface the expanded prompt as a new user
                // message the next turn will see. The SkillTool
                // caller is responsible for tagging
                // `parent_tool_use_id` before pushing to history.
                let expanded_message = coco_messages::create_user_message(&expanded_prompt);
                tracing::info!(
                    skill_name = %skill.name,
                    prompt_chars = expanded_prompt.len(),
                    allow_rules = allow_rules.len(),
                    "skill inline expanded"
                );
                let summary = format!(
                    "Inline skill '{}' expanded ({} chars)",
                    skill.name,
                    expanded_prompt.len()
                );
                let new_messages = vec![
                    serde_json::to_value(&expanded_message).unwrap_or(serde_json::Value::Null),
                ];
                let permission_updates = if allow_rules.is_empty() {
                    Vec::new()
                } else {
                    vec![PermissionUpdate::AddRules {
                        rules: allow_rules,
                        destination: PermissionUpdateDestination::Command,
                    }]
                };
                Ok(SkillInvocationResult::Inline {
                    summary,
                    new_messages,
                    permission_updates,
                })
            }
            SkillContext::Fork => {
                // Fork: route the expanded prompt through the subagent
                // engine with the skill's allowed_tools + model override.
                let engine =
                    self.agent_engine
                        .clone()
                        .ok_or_else(|| SkillInvocationError::Forked {
                            reason: format!(
                                "Skill '{}' is fork-mode but no AgentQueryEngine is installed. \
                             Use QuerySkillRuntime::with_agent_engine at session bootstrap.",
                                skill.name
                            ),
                        })?;

                let agent_id = format!(
                    "skill-{}-{}",
                    skill.name,
                    &uuid::Uuid::new_v4().simple().to_string()[..8]
                );
                let config = AgentQueryConfig {
                    system_prompt: String::new(),
                    identity: coco_tool_runtime::AgentRunIdentity::new(
                        inherit.session_id.clone(),
                        agent_id.clone(),
                        coco_tool_runtime::AgentRunKind::Skill,
                    )
                    .map_err(|reason| SkillInvocationError::Forked { reason })?,
                    model_selection: coco_types::LlmModelSelection::from_model_and_role(
                        skill.model.as_deref(),
                        skill.model_role,
                    ),
                    permission_mode: inherit.permission_mode,
                    permission_prompt_policy:
                        coco_tool_runtime::PermissionPromptPolicy::PromptAllowed,
                    // Skill forks share the parent cwd (no worktree isolation),
                    // so no extra read-scope dirs to inherit.
                    inherited_read_dirs: Vec::new(),
                    max_turns: None,
                    context_window: None,
                    prompt_cache: None,
                    max_output_tokens: None,
                    // Fork skills do NOT narrow the tool registry. The
                    // subagent sees the full inherited toolset;
                    // `extra_permission_rules` below auto-allow the
                    // listed tools, others go through the normal
                    // permission pipeline.
                    allowed_tools: Vec::new(),
                    disallowed_tools: Vec::new(),
                    extra_permission_rules: allow_rules,
                    live_permission_rules: None,
                    live_permission_mode: None,
                    tool_overrides: inherit.tool_overrides.clone(),
                    features: inherit.features.clone(),
                    // Fork-mode skill subagents inherit the gate map
                    // from the dispatch's `SkillGateContext`.
                    skill_overrides: Some(gate.overrides.clone()),
                    parent_tool_filter: inherit.parent_tool_filter.clone(),
                    active_shell_tool: inherit.active_shell_tool,
                    preserve_tool_use_results: false,
                    is_teammate: false,
                    is_in_process_teammate: false,
                    plan_mode_required: false,
                    bypass_permissions_available: false,
                    cwd_override: None,
                    // Skill fork: absent model/model_role inherits the
                    // parent session's Main client via `InheritMain`.
                    fork_context_messages: Vec::new(),
                    allowed_write_roots: Vec::new(),
                    // Skill subagents inherit the parent's call options
                    // — no per-call AgentTool tuning surface today, so
                    // these stay defaults.
                    effort: None,
                    use_exact_tools: false,
                    mcp_servers: Vec::new(),
                    initial_prompt: None,
                    // Skills don't have an AgentDefinition counterpart
                    // (they're a separate first-class workflow type).
                    definition: None,
                    // Skills inherit the parent's permission bridge by
                    // construction (they reuse the parent runtime).
                    // Setting this to `None` keeps the engine factory's
                    // wire_engine path on the parent's bridge.
                    permission_bridge: None,
                    // Skills don't stream into a task buffer.
                    event_tx: None,
                    wire_dump: None,
                    // Skills don't install a per-fork canUseTool
                    // callback — they inherit the parent's permission
                    // pipeline (allow/deny rules + tool's own
                    // `check_permissions`).
                    can_use_tool: None,
                    require_can_use_tool: false,
                    fork_label: None,
                    cancel: None,
                    // Skill forks don't run the AgentSummary timer.
                    live_transcript: None,
                };

                tracing::info!(
                    skill_name = %skill.name,
                    agent_id = %agent_id,
                    extra_permission_rules = config.extra_permission_rules.len(),
                    "skill fork dispatch"
                );
                let query_result = engine
                    .execute_query(&expanded_prompt, config)
                    .await
                    .map_err(|e| {
                        tracing::warn!(
                            skill_name = %skill.name,
                            error = %e,
                            "skill fork failed"
                        );
                        SkillInvocationError::Forked {
                            reason: e.to_string(),
                        }
                    })?;
                let output = query_result
                    .response_text
                    .unwrap_or_else(|| "(no output)".into());
                tracing::info!(
                    skill_name = %skill.name,
                    agent_id = %agent_id,
                    output_chars = output.len(),
                    "skill fork ok"
                );
                Ok(SkillInvocationResult::Forked { agent_id, output })
            }
        };

        // Record usage at the model-invoked path so frequently-used skills
        // surface in the `/` autocomplete's "recently used" section.
        // Record on success only — a failed fork doesn't count.
        // `spawn_blocking` keeps the async dispatcher unblocked when
        // `record` hits its slow path (read + atomic rename).
        if result.is_ok() {
            let recorded_name = skill.name.clone();
            tokio::task::spawn_blocking(move || {
                let config_home = coco_config::global_config::config_home();
                coco_skills::usage::record(&config_home, &recorded_name);
            });
        }
        result
    }

    async fn read_skill_body(
        &self,
        name: &str,
        tiers: &coco_config::SkillOverrideTiers,
    ) -> Option<String> {
        let name = coco_tools::tools::skill_advanced::normalize_skill_name(name);
        let skill = self.manager.get(name)?;
        // Apply the same gate as the listing path so a frontmatter
        // `skills: [foo]` cannot smuggle a disable-model-invocation
        // skill into the subagent's preloaded prompt.
        if skill.disabled || skill.disable_model_invocation {
            return None;
        }
        if coco_skills::effective_skill_state(&skill, tiers) == SkillOverrideState::Off {
            return None;
        }
        let body = skill.prompt.trim();
        if body.is_empty() {
            return None;
        }
        Some(body.to_string())
    }
}

/// Convert a skill's `allowed-tools` list (tool patterns) into
/// Command-source allow rules. Each entry becomes one
/// `PermissionRule` with `source: Command, behavior: Allow` so
/// downstream evaluation matches both inline and fork paths via the
/// same `Command` slot (`alwaysAllowRules.command`).
fn build_command_allow_rules(allowed_tools: &[String]) -> Vec<PermissionRule> {
    allowed_tools
        .iter()
        .map(|raw| PermissionRule {
            source: PermissionRuleSource::Command,
            behavior: PermissionBehavior::Allow,
            value: coco_permissions::parse_rule_string(raw),
        })
        .collect()
}

/// Whether the resolved skill matches *any* `/<word>` token the
/// user typed in the current turn, including aliases. Alias-aware:
/// `SkillTool` always receives the canonical name; this is where
/// the alias bypass actually lives.
fn user_invoked_via_slash(skill: &coco_skills::SkillDefinition, gate: &SkillGateContext) -> bool {
    if gate.typed_slashes_in_turn.contains(skill.name.as_str()) {
        return true;
    }
    skill
        .aliases
        .iter()
        .any(|alias| gate.typed_slashes_in_turn.contains(alias.as_str()))
}

#[cfg(test)]
#[path = "skill_runtime.test.rs"]
mod tests;
