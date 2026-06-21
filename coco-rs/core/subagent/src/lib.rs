//! Pure-logic subagent rules: definition loading, source precedence, built-in
//! catalog, AgentTool prompt rendering, tool filter planning, validation.
//!
//! `tools/AgentTool/loadAgentsDir.ts`, `builtInAgents.ts`,
//! `prompt.ts`, `agentToolUtils.ts`, `built-in/*.ts`.
//!
//! This crate has no tokio, no app state, no QueryEngine. All side effects
//! are synchronous `std::fs` reads triggered by an explicit `load()` call.
//! See `docs/coco-rs/subagent-refactor-plan.md` § D8 for ownership boundary.

pub mod builtin_prompts;
pub mod builtins;
pub mod coordinator_mode;
pub mod definition_store;
pub mod filter;
pub mod fork;
pub mod frontmatter;
pub mod handoff;
pub mod prompt;
pub mod snapshot;
pub mod spawn_resolution;
pub mod subagent_role;
pub mod summary;
pub mod transcript;
pub mod validation;
pub mod writable;

pub use builtin_prompts::{
    CocoGuideDynamicContext, GuideAgentEntry, GuideCommandEntry, coco_guide_dynamic_block,
};
pub use builtins::{BuiltinAgentCatalog, builtin_definition, builtin_definitions};
pub use coordinator_mode::{
    SessionMode, SessionModeSwitch, TaskNotification, TaskNotificationStatus,
    TaskNotificationUsage, coordinator_system_prompt, coordinator_user_context,
    is_coordinator_mode, is_coordinator_mode_env, is_fork_subagent_active,
    render_task_notification, session_mode_switch_action, worker_tool_pool,
};
pub use definition_store::{
    AgentDefinitionStore, AgentLoadReport, AgentSearchPaths, LoadedAgentDefinition,
    SnapshotInspectorFn,
};
pub use filter::{
    AllowedAgentTypes, async_subagent_disallowed_tools, parse_allowed_agent_types,
    parse_tool_allow_list, subagent_disallowed_tools,
};
pub use fork::{
    FORK_BOILERPLATE_TAG, FORK_DIRECTIVE_PREFIX, build_fork_child_message, build_fork_child_rules,
    is_fork_enabled, is_in_fork_child,
};
pub use frontmatter::{
    FrontmatterParseError, parse_agent_markdown, parse_color_value, parse_isolation_value,
    parse_memory_value,
};
pub use handoff::{
    HANDOFF_REVIEW_USER_PROMPT, HandoffClassification, UNAVAILABLE_WARNING,
    build_transcript_summary as build_handoff_transcript_summary, handoff_classifier_active,
    parse_classifier_response, render_block_message, should_classify,
    stage1_prompts as handoff_stage1_prompts, stage2_prompts as handoff_stage2_prompts,
};
pub use prompt::{AgentToolPromptRenderer, PromptOptions, format_tools_description};
pub use snapshot::AgentCatalogSnapshot;
pub use snapshot::has_required_mcp_servers;
pub use spawn_resolution::{SubagentSelection, resolve_subagent_selection};
pub use subagent_role::{resolve_subagent_role, role_for_builtin};
pub use summary::{
    build_summary_prompts, render_transcript_tail, sanitize_summary, should_summarize,
};
pub use transcript::filter_transcript;
pub use validation::{AgentDefinitionValidator, ValidationDiagnostic, ValidationError};
pub use writable::{next_unused_color, resolve_writable_agent_dir};

/// One-shot built-in agent types — `ONE_SHOT_BUILTIN_AGENT_TYPES`
/// (`constants.ts:9-12`). **Case-sensitive** — `"explore"`/`"plan"` do not
/// hit. The set short-circuits the SendMessage continuation trailer in
/// AgentTool result rendering.
pub const ONE_SHOT_BUILTIN_AGENT_TYPES: &[&str] = &["Explore", "Plan"];

/// Empty-content marker injected by AgentTool when the subagent returned
/// no text. **Exact** literal (`AgentTool.tsx:1347-1350`).
pub const EMPTY_AGENT_OUTPUT_MARKER: &str = "(Subagent completed but returned no output.)";

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
