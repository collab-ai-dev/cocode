//! Built-in agent catalog.
//!
//! Each built-in defines `agentType`, `whenToUse`,
//! `tools`/`disallowedTools`, `model`, `color`, `background`,
//! `omitClaudeMd`, and `system_prompt` (the body the model receives).
//! The `name` field doubles as the catalog ID and the lookup key in
//! store snapshots.
//!
//! Built-in `system_prompt` bodies live in [`crate::builtin_prompts`]
//! (one constant or factory per agent). Variants that depend on the
//! host build (`hasEmbeddedSearchTools()`) are threaded through
//! [`BuiltinAgentCatalog::has_embedded_search_tools`].
//!
//! Optional built-ins (Explore, Plan, verification, coco-guide) are
//! gated by booleans on `BuiltinAgentCatalog`; consumers (CLI bootstrap)
//! map these from feature flags / GrowthBook gates.

use coco_types::{
    AgentColorName, AgentDefinition, AgentSource, AgentTypeId, SubagentType, ToolName,
};

use crate::builtin_prompts::{
    STATUSLINE_SETUP_SYSTEM_PROMPT, VERIFICATION_CRITICAL_SYSTEM_REMINDER,
    coco_guide_system_prompt, explore_system_prompt, general_purpose_system_prompt,
    plan_system_prompt, verification_system_prompt,
};

/// What the SDK / CLI / TUI passes in to choose which optional built-ins
/// load.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BuiltinAgentCatalog {
    /// `BUILTIN_EXPLORE_PLAN_AGENTS` + `tengu_amber_stoat`.
    pub include_explore_plan: bool,
    /// `VERIFICATION_AGENT` + `tengu_hive_evidence`.
    pub include_verification: bool,
    /// `coco-guide` is included for non-SDK entrypoints (CLI/TUI).
    pub include_coco_guide: bool,
    /// SDK noninteractive mode disables the entire built-in roster.
    pub disable_all: bool,
    /// Host build embeds search tools (`bfs` / `ugrep`) into the Bash
    /// tool. When true, `coco-guide`'s default tool list swaps
    /// the dedicated `Glob` / `Grep` tools for `Bash`. coco-rs is a
    /// 3p build by default — leave this `false` unless the host
    /// explicitly disabled the `Glob`/`Grep` tools.
    pub has_embedded_search_tools: bool,
}

impl BuiltinAgentCatalog {
    /// Sensible defaults for an interactive CLI/TUI session.
    pub fn interactive() -> Self {
        Self {
            include_explore_plan: true,
            include_verification: false,
            include_coco_guide: true,
            disable_all: false,
            has_embedded_search_tools: false,
        }
    }

    /// SDK noninteractive mode (`CLAUDE_AGENT_SDK_DISABLE_BUILTIN_AGENTS`).
    /// Disables the entire built-in roster — caller may then inject
    /// extension built-ins via `AgentDefinitionStore::insert_definition`.
    pub fn sdk_noninteractive() -> Self {
        Self {
            disable_all: true,
            ..Self::default()
        }
    }

    /// All built-ins enabled. Useful for snapshots and tests.
    pub fn all_enabled() -> Self {
        Self {
            include_explore_plan: true,
            include_verification: true,
            include_coco_guide: true,
            disable_all: false,
            has_embedded_search_tools: false,
        }
    }
}

/// Resolve the built-in roster for a session.
pub fn builtin_definitions(catalog: BuiltinAgentCatalog) -> Vec<AgentDefinition> {
    if catalog.disable_all {
        return Vec::new();
    }

    let embedded = catalog.has_embedded_search_tools;
    let mut out = Vec::with_capacity(6);
    out.push(general_purpose());
    out.push(statusline_setup());
    if catalog.include_explore_plan {
        out.push(explore(embedded));
        out.push(plan(embedded));
    }
    if catalog.include_verification {
        out.push(verification());
    }
    if catalog.include_coco_guide {
        out.push(coco_guide_with(embedded));
    }
    out
}

/// Lookup a single built-in by canonical (case-sensitive) `agent_type`.
///
/// The lookup has no `BuiltinAgentCatalog` context, so it defaults to the
/// non-embedded variant (3p build). Embedded-host callers should iterate
/// [`builtin_definitions`] with the catalog flag set instead.
pub fn builtin_definition(agent_type: &str) -> Option<AgentDefinition> {
    SubagentType::ALL.iter().find_map(|s| {
        if s.as_str() == agent_type {
            Some(builtin_for(*s, false))
        } else {
            None
        }
    })
}

fn builtin_for(t: SubagentType, has_embedded_search_tools: bool) -> AgentDefinition {
    match t {
        SubagentType::GeneralPurpose => general_purpose(),
        SubagentType::StatusLine => statusline_setup(),
        SubagentType::Explore => explore(has_embedded_search_tools),
        SubagentType::Plan => plan(has_embedded_search_tools),
        SubagentType::Verification => verification(),
        SubagentType::CocoGuide => coco_guide_with(has_embedded_search_tools),
    }
}

// ── individual built-ins ──
//
// `system_prompt` is the body the model receives — sourced from
// [`crate::builtin_prompts`].

fn base(t: SubagentType, when_to_use: &str) -> AgentDefinition {
    AgentDefinition {
        agent_type: AgentTypeId::Builtin(t),
        name: t.as_str().to_owned(),
        when_to_use: Some(when_to_use.to_owned()),
        description: Some(when_to_use.to_owned()),
        source: AgentSource::BuiltIn,
        base_dir: Some("built-in".to_owned()),
        ..Default::default()
    }
}

fn general_purpose() -> AgentDefinition {
    AgentDefinition {
        // `tools: ['*']` means "all tools". coco-rs encodes wildcard
        // as the empty list (see `AgentToolFilter::plan`
        // `uses_default_allow_list`).
        system_prompt: Some(general_purpose_system_prompt()),
        ..base(
            SubagentType::GeneralPurpose,
            "General-purpose agent for researching complex questions, searching for code, and executing multi-step tasks. When you are searching for a keyword or file and are not confident that you will find the right match in the first few tries use this agent to perform the search for you.",
        )
    }
}

fn read_only_disallowed() -> Vec<String> {
    // These are real ToolName variants — `Edit`/`Write`, NOT `FileEdit`/`FileWrite`.
    vec![
        ToolName::Agent.as_str().into(),
        ToolName::ExitPlanMode.as_str().into(),
        ToolName::Edit.as_str().into(),
        ToolName::Write.as_str().into(),
        ToolName::NotebookEdit.as_str().into(),
    ]
}

fn statusline_setup() -> AgentDefinition {
    AgentDefinition {
        model_role: Some(coco_types::ModelRole::Main),
        color: Some(AgentColorName::Orange),
        allowed_tools: coco_types::ToolAllowList::Explicit(vec![
            ToolName::Read.as_str().into(),
            ToolName::Edit.as_str().into(),
        ]),
        system_prompt: Some(STATUSLINE_SETUP_SYSTEM_PROMPT.into()),
        ..base(
            SubagentType::StatusLine,
            "Use this agent to configure the user's Coco status line setting.",
        )
    }
}

fn explore(has_embedded_search_tools: bool) -> AgentDefinition {
    AgentDefinition {
        omit_claude_md: true,
        disallowed_tools: read_only_disallowed(),
        system_prompt: Some(explore_system_prompt(has_embedded_search_tools)),
        ..base(
            SubagentType::Explore,
            "Fast agent specialized for exploring codebases. Use this when you need to quickly find files by patterns (eg. \"src/components/**/*.tsx\"), search code for keywords (eg. \"API endpoints\"), or answer questions about the codebase (eg. \"how do API endpoints work?\"). When calling this agent, specify the desired thoroughness level: \"quick\" for basic searches, \"medium\" for moderate exploration, or \"very thorough\" for comprehensive analysis across multiple locations and naming conventions.",
        )
    }
}

fn plan(has_embedded_search_tools: bool) -> AgentDefinition {
    AgentDefinition {
        omit_claude_md: true,
        disallowed_tools: read_only_disallowed(),
        system_prompt: Some(plan_system_prompt(has_embedded_search_tools)),
        ..base(
            SubagentType::Plan,
            "Software architect agent for designing implementation plans. Use this when you need to plan the implementation strategy for a task. Returns step-by-step plans, identifies critical files, and considers architectural trade-offs.",
        )
    }
}

fn verification() -> AgentDefinition {
    AgentDefinition {
        color: Some(AgentColorName::Red),
        background: true,
        disallowed_tools: read_only_disallowed(),
        system_prompt: Some(verification_system_prompt()),
        // `criticalSystemReminder_EXPERIMENTAL`.
        critical_system_reminder: Some(VERIFICATION_CRITICAL_SYSTEM_REMINDER.into()),
        ..base(
            SubagentType::Verification,
            "Use this agent to verify that implementation work is correct before reporting completion. Invoke after non-trivial tasks (3+ file edits, backend/API changes, infrastructure changes). Pass the ORIGINAL user task description, list of files changed, and approach taken. The agent runs builds, tests, linters, and checks to produce a PASS/FAIL/PARTIAL verdict with evidence.",
        )
    }
}

/// When `has_embedded_search_tools` is true, the host build aliases
/// `Glob`/`Grep` to `Bash` (with embedded `bfs` / `ugrep`), so the
/// agent's tool list swaps the dedicated tools for `Bash`. coco-rs is
/// a 3p build by default; the flag is plumbed through
/// `BuiltinAgentCatalog::has_embedded_search_tools`.
///
/// The system prompt embeds the same flag — see
/// [`crate::builtin_prompts::coco_guide_system_prompt`]. The dynamic
/// Dynamic context sections appended at spawn time (custom skills, custom
/// agents, MCP servers, plugin commands, settings.json) are not folded
/// in here — they belong on the spawn-time prompt assembler, not on
/// the static catalog entry.
///
/// **Coco-rs rename**: this agent was originally `claude-code-guide`;
/// coco-rs owns the identifier as `coco-guide` (see
/// [`coco_types::SubagentType::CocoGuide`]).
fn coco_guide_with(has_embedded_search_tools: bool) -> AgentDefinition {
    let allowed_tools = if has_embedded_search_tools {
        coco_types::ToolAllowList::Explicit(vec![
            ToolName::Bash.as_str().into(),
            ToolName::Read.as_str().into(),
            ToolName::WebFetch.as_str().into(),
            ToolName::WebSearch.as_str().into(),
        ])
    } else {
        coco_types::ToolAllowList::Explicit(vec![
            ToolName::Glob.as_str().into(),
            ToolName::Grep.as_str().into(),
            ToolName::Read.as_str().into(),
            ToolName::WebFetch.as_str().into(),
            ToolName::WebSearch.as_str().into(),
        ])
    };
    AgentDefinition {
        model_role: Some(coco_types::ModelRole::Explore),
        permission_mode: Some("dontAsk".into()),
        allowed_tools,
        system_prompt: Some(coco_guide_system_prompt(has_embedded_search_tools)),
        ..base(
            SubagentType::CocoGuide,
            // The agent identifier is renamed to `coco-guide` for
            // coco-rs. `SendMessage` is inlined as the literal value.
            "Use this agent when the user asks questions (\"Can Claude...\", \"Does Claude...\", \"How do I...\") about: (1) Claude Code (the CLI tool) - features, hooks, slash commands, MCP servers, settings, IDE integrations, keyboard shortcuts; (2) Claude Agent SDK - building custom agents; (3) Claude API (formerly Anthropic API) - API usage, tool use, Anthropic SDK usage. **IMPORTANT:** Before spawning a new agent, check if there is already a running or recently completed coco-guide agent that you can continue via SendMessage.",
        )
    }
}
