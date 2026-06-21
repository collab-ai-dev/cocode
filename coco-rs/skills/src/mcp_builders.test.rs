//! Tests for `mcp_builders`.

use super::*;
use crate::SkillManager;
use crate::SkillSource;

fn spec(content: &str) -> McpSkillSpec {
    McpSkillSpec {
        server_name: "test-server".to_string(),
        uri: "skill://test-server/example".to_string(),
        name: "example".to_string(),
        description: Some("Example skill from server metadata".to_string()),
        content: content.to_string(),
    }
}

#[test]
fn default_builder_uses_frontmatter_description_when_present() {
    let s = spec(
        "---\n\
         description: Frontmatter description wins\n\
         ---\n\
         Body content here.",
    );
    let skill = DefaultMcpSkillBuilder.build(&s).expect("build");
    assert_eq!(skill.description, "Frontmatter description wins");
    assert!(skill.has_user_specified_description);
}

#[test]
fn default_builder_falls_back_to_spec_description() {
    let s = spec("Body content with no frontmatter.");
    let skill = DefaultMcpSkillBuilder.build(&s).expect("build");
    assert_eq!(skill.description, "Example skill from server metadata");
    assert!(skill.has_user_specified_description);
}

#[test]
fn default_builder_falls_back_to_body_extraction_when_no_description() {
    let s = McpSkillSpec {
        server_name: "srv".into(),
        uri: "skill://srv/x".into(),
        name: "x".into(),
        description: None,
        content: "# Heading line\nSecond line".into(),
    };
    let skill = DefaultMcpSkillBuilder.build(&s).expect("build");
    // extract_description_from_markdown strips the `#` and uses first non-empty line.
    assert_eq!(skill.description, "Heading line");
    assert!(!skill.has_user_specified_description);
}

#[test]
fn default_builder_emits_mcp_source() {
    let s = spec("---\ndescription: x\n---\nbody");
    let skill = DefaultMcpSkillBuilder.build(&s).expect("build");
    match skill.source {
        SkillSource::Mcp { server_name } => assert_eq!(server_name, "test-server"),
        other => panic!("expected SkillSource::Mcp, got {other:?}"),
    }
}

#[test]
fn default_builder_parses_allowed_tools_and_aliases() {
    let s = spec(
        "---\n\
         description: x\n\
         allowed-tools: Bash, Read\n\
         aliases: ex, sample\n\
         ---\n\
         body",
    );
    let skill = DefaultMcpSkillBuilder.build(&s).expect("build");
    assert_eq!(
        skill.allowed_tools.as_deref(),
        Some(["Bash".to_string(), "Read".to_string()].as_slice())
    );
    assert_eq!(skill.aliases, vec!["ex".to_string(), "sample".to_string()]);
}

#[test]
fn default_builder_paths_are_always_empty_for_mcp_skills() {
    // Even if frontmatter declares paths, MCP skills opt out (no on-disk
    // file matching makes sense for a server-published skill).
    let s = spec(
        "---\n\
         description: x\n\
         paths: src/**/*.ts\n\
         ---\n\
         body",
    );
    let skill = DefaultMcpSkillBuilder.build(&s).expect("build");
    assert!(skill.paths.is_empty());
}

#[test]
fn register_mcp_skill_round_trip() {
    let manager = SkillManager::new();
    let s = spec("---\ndescription: round-trip\n---\nbody");
    manager.register_mcp_skill(s).expect("register");

    let listed = manager.all();
    assert_eq!(listed.len(), 1);
    let s = &listed[0];
    assert_eq!(s.name, "example");
    assert!(
        matches!(s.source, SkillSource::Mcp { ref server_name } if server_name == "test-server")
    );
}

#[test]
fn register_multiple_servers_and_unregister_one() {
    let manager = SkillManager::new();

    let mut sa = spec("---\ndescription: a\n---\nbody");
    sa.server_name = "server-a".into();
    sa.name = "alpha".into();
    manager.register_mcp_skill(sa).unwrap();

    let mut sb = spec("---\ndescription: b\n---\nbody");
    sb.server_name = "server-b".into();
    sb.name = "beta".into();
    manager.register_mcp_skill(sb).unwrap();

    assert_eq!(manager.len(), 2);

    let removed = manager.unregister_skills_for_mcp_server("server-a");
    assert_eq!(removed, 1);
    assert_eq!(manager.len(), 1);

    let remaining = manager.all();
    assert_eq!(remaining[0].name, "beta");
}

#[test]
fn re_registering_same_key_replaces() {
    let manager = SkillManager::new();
    let s1 = spec("---\ndescription: first\n---\nfirst body");
    manager.register_mcp_skill(s1).unwrap();
    let s2 = spec("---\ndescription: second\n---\nsecond body");
    // Same (server_name, name) as `s1` (default `spec` builder).
    manager.register_mcp_skill(s2).unwrap();
    assert_eq!(manager.len(), 1);
    let listed = manager.all();
    assert_eq!(listed[0].description, "second");

    // Lookup by name surfaces the MCP-sourced skill.
    let got = manager.get("example").expect("get");
    assert_eq!(got.description, "second");
}

#[test]
fn on_disk_skill_wins_over_mcp_on_name_collision() {
    let manager = SkillManager::new();
    // On-disk: pre-load a synthetic SkillDefinition by directly calling
    // register().
    let on_disk = SkillDefinition {
        name: "example".into(),
        display_name: None,
        description: "disk wins".into(),
        prompt: "disk".into(),
        source: SkillSource::Bundled,
        aliases: Vec::new(),
        allowed_tools: None,
        model: None,
        model_role: None,
        when_to_use: None,
        argument_names: Vec::new(),
        paths: Vec::new(),
        effort: None,
        context: SkillContext::Inline,
        agent: None,
        version: None,
        disabled: false,
        hooks: None,
        argument_hint: None,
        user_invocable: true,
        disable_model_invocation: false,
        shell: None,
        content_length: 0,
        is_hidden: false,
        has_user_specified_description: true,
        progress_message: None,
        gated_by: None,
        files: std::collections::HashMap::new(),
        skill_root: None,
    };
    manager.register(on_disk);

    // MCP-sourced with same name.
    let s = spec("---\ndescription: mcp loses\n---\nbody");
    manager.register_mcp_skill(s).unwrap();

    let resolved = manager.get("example").expect("get");
    assert_eq!(resolved.description, "disk wins");
}

#[test]
fn register_mcp_skill_builder_is_no_op_when_already_set() {
    // First registration may or may not succeed depending on test
    // execution order; we don't assert the boolean here. What matters
    // is that two consecutive calls don't panic and that
    // `mcp_skill_builder()` returns a builder either way.
    let _ = register_mcp_skill_builder(Arc::new(DefaultMcpSkillBuilder));
    let _ = register_mcp_skill_builder(Arc::new(DefaultMcpSkillBuilder));
    let _ = mcp_skill_builder();
}
