use pretty_assertions::assert_eq;

use super::*;

#[test]
fn test_subagent_type_canonical_case() {
    // Explore/Plan are PascalCase; the rest are kebab-case lowercase.
    // Output side must always emit canonical case.
    assert_eq!(SubagentType::Explore.as_str(), "Explore");
    assert_eq!(SubagentType::Plan.as_str(), "Plan");
    assert_eq!(SubagentType::GeneralPurpose.as_str(), "general-purpose");
    assert_eq!(SubagentType::StatusLine.as_str(), "statusline-setup");
    assert_eq!(SubagentType::Verification.as_str(), "verification");
    assert_eq!(SubagentType::CocoGuide.as_str(), "coco-guide");
}

#[test]
fn test_subagent_type_aliases_canonicalize_on_input() {
    // Input side accepts lowercase aliases for `Explore`/`Plan` and
    // underscore variants — but they all canonicalize to the same variant.
    assert_eq!(
        SubagentType::from_str("Explore").unwrap(),
        SubagentType::Explore
    );
    assert_eq!(
        SubagentType::from_str("explore").unwrap(),
        SubagentType::Explore
    );
    assert_eq!(SubagentType::from_str("Plan").unwrap(), SubagentType::Plan);
    assert_eq!(SubagentType::from_str("plan").unwrap(), SubagentType::Plan);
    assert_eq!(
        SubagentType::from_str("statusline-setup").unwrap(),
        SubagentType::StatusLine
    );
    assert_eq!(
        SubagentType::from_str("statusline_setup").unwrap(),
        SubagentType::StatusLine
    );
    // Coco-rs canonical name + snake_case alias.
    assert_eq!(
        SubagentType::from_str("coco-guide").unwrap(),
        SubagentType::CocoGuide
    );
    assert_eq!(
        SubagentType::from_str("coco_guide").unwrap(),
        SubagentType::CocoGuide
    );
    // The legacy TS string is intentionally NOT accepted — coco-rs
    // owns its own identifier (CLAUDE.md "no backward-compat shims").
    assert!(SubagentType::from_str("claude-code-guide").is_err());
    assert_eq!(SubagentType::CocoGuide.as_str(), "coco-guide");
}

#[test]
fn test_subagent_type_serde_uses_canonical_case() {
    // Round-trips through canonical case, even when the input was an alias.
    let parsed = SubagentType::from_str("explore").unwrap();
    let json = serde_json::to_string(&parsed).unwrap();
    assert_eq!(json, "\"Explore\"");
    let again: SubagentType = serde_json::from_str(&json).unwrap();
    assert_eq!(again, SubagentType::Explore);
}

#[test]
fn test_agent_type_id_builtin() {
    let id: AgentTypeId = "Explore".parse().unwrap();
    assert_eq!(id, AgentTypeId::Builtin(SubagentType::Explore));
    assert_eq!(id.to_string(), "Explore");
}

#[test]
fn test_agent_type_id_custom() {
    let id: AgentTypeId = "my-custom-agent".parse().unwrap();
    assert_eq!(id, AgentTypeId::Custom("my-custom-agent".into()));
}

#[test]
fn test_agent_type_id_serde_roundtrip() {
    let id = AgentTypeId::Builtin(SubagentType::Plan);
    let json = serde_json::to_string(&id).unwrap();
    assert_eq!(json, "\"Plan\"");
    let parsed: AgentTypeId = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, id);
}

// ── AgentSource ──

#[test]
fn test_agent_source_priority_order() {
    use AgentSource::*;
    assert!(BuiltIn.priority() < Plugin.priority());
    assert!(Plugin.priority() < UserSettings.priority());
    assert!(UserSettings.priority() < ProjectSettings.priority());
    assert!(ProjectSettings.priority() < FlagSettings.priority());
    assert!(FlagSettings.priority() < PolicySettings.priority());
}

#[test]
fn test_agent_source_serde_uses_ts_strings() {
    // TS uses `built-in`, `plugin`, `userSettings`, `projectSettings`,
    // `flagSettings`, `policySettings` — these are wire-stable.
    for (variant, expected) in [
        (AgentSource::BuiltIn, "\"built-in\""),
        (AgentSource::Plugin, "\"plugin\""),
        (AgentSource::UserSettings, "\"userSettings\""),
        (AgentSource::ProjectSettings, "\"projectSettings\""),
        (AgentSource::FlagSettings, "\"flagSettings\""),
        (AgentSource::PolicySettings, "\"policySettings\""),
    ] {
        let json = serde_json::to_string(&variant).unwrap();
        assert_eq!(json, expected);
        let parsed: AgentSource = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, variant);
    }
}

// ── AgentColorName ──

#[test]
fn test_agent_color_name_serde_lowercase() {
    for (variant, expected) in [
        (AgentColorName::Red, "\"red\""),
        (AgentColorName::Cyan, "\"cyan\""),
    ] {
        let json = serde_json::to_string(&variant).unwrap();
        assert_eq!(json, expected);
        let parsed: AgentColorName = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, variant);
    }
}

#[test]
fn test_agent_color_name_from_str_rejects_unknown() {
    assert_eq!(
        AgentColorName::from_str("Red").unwrap(),
        AgentColorName::Red
    );
    assert_eq!(
        AgentColorName::from_str("cyan").unwrap(),
        AgentColorName::Cyan
    );
    assert!(AgentColorName::from_str("magenta").is_err());
}

// ── AgentIsolation ──

#[test]
fn test_agent_isolation_serde_roundtrip() {
    for (variant, expected_json) in [
        (AgentIsolation::None, "\"none\""),
        (AgentIsolation::Worktree, "\"worktree\""),
        (AgentIsolation::Remote, "\"remote\""),
    ] {
        let json = serde_json::to_string(&variant).unwrap();
        assert_eq!(json, expected_json);
        let parsed: AgentIsolation = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, variant);
    }
}

#[test]
fn test_agent_isolation_from_str() {
    assert_eq!(
        AgentIsolation::from_str("none").unwrap(),
        AgentIsolation::None
    );
    assert_eq!(
        AgentIsolation::from_str("worktree").unwrap(),
        AgentIsolation::Worktree
    );
    assert_eq!(
        AgentIsolation::from_str("remote").unwrap(),
        AgentIsolation::Remote
    );
    assert!(AgentIsolation::from_str("invalid").is_err());
}

#[test]
fn test_agent_isolation_default() {
    assert_eq!(AgentIsolation::default(), AgentIsolation::None);
}

// ── MemoryScope ──

#[test]
fn test_memory_scope_serde_roundtrip() {
    for (variant, expected_json) in [
        (MemoryScope::User, "\"user\""),
        (MemoryScope::Project, "\"project\""),
        (MemoryScope::Local, "\"local\""),
    ] {
        let json = serde_json::to_string(&variant).unwrap();
        assert_eq!(json, expected_json);
        let parsed: MemoryScope = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, variant);
    }
}

#[test]
fn test_memory_scope_from_str() {
    assert_eq!(MemoryScope::from_str("user").unwrap(), MemoryScope::User);
    assert_eq!(
        MemoryScope::from_str("project").unwrap(),
        MemoryScope::Project
    );
    assert_eq!(MemoryScope::from_str("local").unwrap(), MemoryScope::Local);
    assert!(MemoryScope::from_str("global").is_err());
}

#[test]
fn test_memory_scope_default() {
    assert_eq!(MemoryScope::default(), MemoryScope::Project);
}

// ── ModelInheritance ──

#[test]
fn test_model_inheritance_serde_roundtrip() {
    let inheritance = ModelInheritance {
        model: "opus-4".into(),
        source: ModelSource::Param,
    };
    let json = serde_json::to_string(&inheritance).unwrap();
    let parsed: ModelInheritance = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, inheritance);
}

#[test]
fn test_model_source_variants() {
    for (variant, expected) in [
        (ModelSource::Param, "param"),
        (ModelSource::Definition, "definition"),
        (ModelSource::Parent, "parent"),
    ] {
        assert_eq!(variant.to_string(), expected);
    }
}

// ── AgentDefinition ──

#[test]
fn test_agent_definition_default() {
    let def = AgentDefinition::default();
    assert!(!def.use_exact_tools);
    assert_eq!(def.isolation, AgentIsolation::None);
    assert_eq!(def.source, AgentSource::BuiltIn);
    assert!(def.mcp_servers.is_empty());
    assert!(def.disallowed_tools.is_empty());
    assert!(def.allowed_tools.is_wildcard());
    assert!(def.effort.is_none());
    assert!(def.model.is_none());
    assert!(def.memory_scope.is_none());
    assert!(def.initial_prompt.is_none());
    assert!(def.system_prompt.is_none());
    assert!(def.max_turns.is_none());
    assert!(def.color.is_none());
    assert!(def.critical_system_reminder.is_none());
}

#[test]
fn test_agent_definition_serde_roundtrip() {
    let def = AgentDefinition {
        agent_type: AgentTypeId::Builtin(SubagentType::Explore),
        name: "researcher".into(),
        when_to_use: Some("Explores the codebase".into()),
        description: Some("Explores the codebase".into()),
        source: AgentSource::ProjectSettings,
        filename: Some("researcher.md".into()),
        base_dir: Some(".coco/agents".into()),
        system_prompt: Some("You are a code researcher.".into()),
        effort: Some(crate::ReasoningEffort::High),
        use_exact_tools: true,
        model: Some("anthropic/claude-opus-4-7".into()),
        isolation: AgentIsolation::Worktree,
        memory_scope: Some(MemoryScope::Project),
        mcp_servers: vec![
            AgentMcpServerSpec::Name("github".into()),
            AgentMcpServerSpec::Name("jira".into()),
        ],
        initial_prompt: Some("Search the codebase for patterns.".into()),
        max_turns: Some(10),
        disallowed_tools: vec!["Bash".into()],
        allowed_tools: ToolAllowList::Explicit(vec!["Read".into(), "Grep".into()]),
        identity: Some("You are a code researcher.".into()),
        color: Some(AgentColorName::Blue),
        ..Default::default()
    };

    let json = serde_json::to_string(&def).unwrap();
    let parsed: AgentDefinition = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.agent_type, def.agent_type);
    assert_eq!(parsed.name, "researcher");
    assert_eq!(parsed.when_to_use.as_deref(), Some("Explores the codebase"));
    assert_eq!(parsed.source, AgentSource::ProjectSettings);
    assert_eq!(parsed.filename.as_deref(), Some("researcher.md"));
    assert_eq!(
        parsed.system_prompt.as_deref(),
        Some("You are a code researcher.")
    );
    assert_eq!(parsed.effort, Some(crate::ReasoningEffort::High));
    assert!(parsed.use_exact_tools);
    assert_eq!(parsed.model.as_deref(), Some("anthropic/claude-opus-4-7"));
    assert_eq!(parsed.isolation, AgentIsolation::Worktree);
    assert_eq!(parsed.memory_scope, Some(MemoryScope::Project));
    assert_eq!(
        parsed.mcp_servers,
        vec![
            AgentMcpServerSpec::Name("github".into()),
            AgentMcpServerSpec::Name("jira".into()),
        ]
    );
    assert_eq!(parsed.max_turns, Some(10));
    assert_eq!(parsed.disallowed_tools, vec!["Bash"]);
    assert_eq!(
        parsed.allowed_tools,
        ToolAllowList::Explicit(vec!["Read".into(), "Grep".into()])
    );
    assert_eq!(parsed.color, Some(AgentColorName::Blue));
}

// ── ToolAllowList from_frontmatter parity matrix ──

#[test]
fn test_tool_allow_list_from_frontmatter_matrix() {
    // TS parity matrix from `parseAgentToolsFromFrontmatter`
    // (`utils/markdownConfigLoader.ts:113-126`).
    //
    // The "missing key" case is *not* exercised here — it bypasses
    // `from_frontmatter` entirely (callers do `.unwrap_or_default()`
    // on `Option<Vec<String>>`, yielding `Wildcard`). All four rows
    // below are the inputs `from_frontmatter` actually sees.
    assert_eq!(
        ToolAllowList::from_frontmatter(vec!["*".into()]),
        ToolAllowList::Wildcard,
        "tools: ['*'] must collapse to Wildcard (TS undefined)"
    );
    assert_eq!(
        ToolAllowList::from_frontmatter(vec![" * ".into()]),
        ToolAllowList::Wildcard,
        "tools: [' * '] (whitespace around star) must collapse to Wildcard"
    );
    assert_eq!(
        ToolAllowList::from_frontmatter(vec![]),
        ToolAllowList::Explicit(vec![]),
        "tools: [] must preserve as Explicit(empty) — TS parity for \
         auto-memory injection (loadAgentsDir.ts:455 gates on \
         `tools !== undefined`, NOT on length > 0)"
    );
    assert_eq!(
        ToolAllowList::from_frontmatter(vec!["Read".into(), "Edit".into()]),
        ToolAllowList::Explicit(vec!["Read".into(), "Edit".into()]),
        "tools: [Read, Edit] must produce Explicit(list)"
    );
}

#[test]
fn test_tool_allow_list_explicit_empty_serializes_as_empty_array() {
    // Wire shape parity: `Explicit(vec![])` round-trips as `[]`. This
    // is what TS sees on disk when an agent author writes `tools: []`
    // — and it's what TS reads back to drive the auto-memory inject
    // gate.
    let list = ToolAllowList::Explicit(vec![]);
    let json = serde_json::to_string(&list).unwrap();
    assert_eq!(json, "[]");
    let parsed: ToolAllowList = serde_json::from_str("[]").unwrap();
    assert_eq!(parsed, ToolAllowList::Explicit(vec![]));
    assert!(!parsed.is_wildcard());
}

#[test]
fn test_agent_definition_serde_skip_empty_defaults() {
    let def = AgentDefinition::default();
    let json = serde_json::to_string(&def).unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    let obj = value.as_object().unwrap();

    // Optional/empty fields should be omitted
    assert!(!obj.contains_key("when_to_use"));
    assert!(!obj.contains_key("description"));
    assert!(!obj.contains_key("filename"));
    assert!(!obj.contains_key("base_dir"));
    assert!(!obj.contains_key("system_prompt"));
    assert!(!obj.contains_key("effort"));
    assert!(!obj.contains_key("model"));
    assert!(!obj.contains_key("memory_scope"));
    assert!(!obj.contains_key("mcp_servers"));
    assert!(!obj.contains_key("initial_prompt"));
    assert!(!obj.contains_key("max_turns"));
    assert!(!obj.contains_key("disallowed_tools"));
    assert!(!obj.contains_key("allowed_tools"));
    assert!(!obj.contains_key("identity"));
    assert!(!obj.contains_key("color"));
    assert!(!obj.contains_key("critical_system_reminder"));
}
