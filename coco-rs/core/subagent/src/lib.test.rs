use coco_types::{AgentColorName, AgentSource, SubagentType, ToolName};
use pretty_assertions::assert_eq;
use std::path::PathBuf;
use tempfile::TempDir;

use super::*;
use crate::builtins::BuiltinAgentCatalog;
use crate::definition_store::{AgentDefinitionStore, AgentSearchPaths, PluginAgentDir};
use crate::filter::parse_allowed_agent_types;
use crate::prompt::{AgentToolPromptRenderer, PromptOptions, format_tools_description};

// ── builtin catalog ──

#[test]
fn builtin_catalog_includes_required_when_enabled() {
    let catalog = BuiltinAgentCatalog::all_enabled();
    let defs = builtins::builtin_definitions(catalog);
    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "general-purpose",
            "statusline-setup",
            "Explore",
            "Plan",
            "verification",
            "coco-guide",
        ]
    );
}

#[test]
fn builtin_catalog_excludes_optional_when_disabled() {
    let defs = builtins::builtin_definitions(BuiltinAgentCatalog::default());
    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, vec!["general-purpose", "statusline-setup"]);
}

#[test]
fn builtin_catalog_disable_all_returns_empty() {
    let catalog = BuiltinAgentCatalog {
        disable_all: true,
        ..BuiltinAgentCatalog::all_enabled()
    };
    assert!(builtins::builtin_definitions(catalog).is_empty());
}

#[test]
fn builtin_definition_lookup_uses_canonical_case() {
    // PascalCase Explore must hit; lowercase must not.
    assert!(builtins::builtin_definition("Explore").is_some());
    assert!(builtins::builtin_definition("explore").is_none());
    assert!(builtins::builtin_definition("verification").is_some());
    assert!(builtins::builtin_definition("Verification").is_none());
}

#[test]
fn explore_built_in_omits_claude_md_and_blocks_writes() {
    let def = builtins::builtin_definition("Explore").unwrap();
    assert!(def.omit_claude_md);
    assert_eq!(def.model, None);
    assert_eq!(def.model_role, None);
    // `exploreAgent.ts:67-73` uses FILE_EDIT_TOOL_NAME = "Edit" and
    // FILE_WRITE_TOOL_NAME = "Write" — NOT "FileEdit"/"FileWrite".
    // Tool names go through `ToolName::*::as_str()` so renames stay
    // consistent.
    for blocked in [
        ToolName::Edit.as_str(),
        ToolName::Write.as_str(),
        ToolName::NotebookEdit.as_str(),
        ToolName::Agent.as_str(),
        ToolName::ExitPlanMode.as_str(),
    ] {
        assert!(
            def.disallowed_tools.iter().any(|t| t == blocked),
            "Explore should block {blocked}; actual: {:?}",
            def.disallowed_tools
        );
    }
}

#[test]
fn coco_guide_uses_dont_ask_permission_mode() {
    // `claudeCodeGuideAgent.ts:120` sets permissionMode: 'dontAsk' so the
    // guide can run its allow-listed tools without prompting.
    let def = builtins::builtin_definition("coco-guide").unwrap();
    assert_eq!(def.permission_mode.as_deref(), Some("dontAsk"));
    assert_eq!(
        def.allowed_tools,
        coco_types::ToolAllowList::Explicit(vec![
            ToolName::Glob.as_str().into(),
            ToolName::Grep.as_str().into(),
            ToolName::Read.as_str().into(),
            ToolName::WebFetch.as_str().into(),
            ToolName::WebSearch.as_str().into(),
        ])
    );
}

#[test]
fn verification_built_in_runs_in_background_with_red_color() {
    let def = builtins::builtin_definition("verification").unwrap();
    assert!(def.background);
    assert_eq!(def.color, Some(AgentColorName::Red));
}

#[test]
fn statusline_built_in_uses_orange_color_and_main_role() {
    let def = builtins::builtin_definition("statusline-setup").unwrap();
    assert_eq!(def.color, Some(AgentColorName::Orange));
    assert_eq!(def.model, None);
    assert_eq!(def.model_role, Some(coco_types::ModelRole::Main));
    assert_eq!(
        def.allowed_tools,
        coco_types::ToolAllowList::Explicit(vec![
            ToolName::Read.as_str().into(),
            ToolName::Edit.as_str().into(),
        ])
    );
}

#[test]
fn every_builtin_definition_carries_a_system_prompt() {
    // Without `system_prompt`, the spawn path falls through to the
    // engine's generic default and the agent loses its role
    // instructions. Pin all six built-ins.
    for name in [
        "general-purpose",
        "statusline-setup",
        "Explore",
        "Plan",
        "verification",
        "coco-guide",
    ] {
        let def =
            builtins::builtin_definition(name).unwrap_or_else(|| panic!("missing built-in {name}"));
        let body = def
            .system_prompt
            .as_deref()
            .unwrap_or_else(|| panic!("{name} must declare system_prompt"));
        assert!(
            body.len() > 200,
            "{name} system_prompt looks suspiciously short ({} bytes)",
            body.len()
        );
    }
}

#[test]
fn verification_carries_critical_system_reminder() {
    // `verificationAgent.ts:150-151` pins this reminder on the
    // agent definition so the per-turn `<system-reminder>` injector
    // can re-emit it.
    let def = builtins::builtin_definition("verification").unwrap();
    let reminder = def.critical_system_reminder.as_deref().unwrap();
    assert!(reminder.starts_with("CRITICAL: This is a VERIFICATION-ONLY task."));
    assert!(reminder.contains("VERDICT: PASS"));
}

#[test]
fn explore_built_in_system_prompt_swaps_for_embedded_search() {
    use crate::builtins::BuiltinAgentCatalog;
    let glob = ToolName::Glob.as_str();
    let bash = ToolName::Bash.as_str();
    // Default 3p build → Glob/Grep wording.
    let defs = builtins::builtin_definitions(BuiltinAgentCatalog::all_enabled());
    let def_default = defs.iter().find(|d| d.name == "Explore").unwrap();
    assert!(
        def_default
            .system_prompt
            .as_deref()
            .unwrap()
            .contains(&format!("- Use {glob} for broad file pattern matching"))
    );

    // Embedded host → find/grep via Bash wording.
    let embedded = BuiltinAgentCatalog {
        has_embedded_search_tools: true,
        ..BuiltinAgentCatalog::all_enabled()
    };
    let defs = builtins::builtin_definitions(embedded);
    let def_embedded = defs.iter().find(|d| d.name == "Explore").unwrap();
    assert!(
        def_embedded
            .system_prompt
            .as_deref()
            .unwrap()
            .contains(&format!("- Use `find` via {bash}"))
    );
}

// ── one-shot constants ──

#[test]
fn one_shot_set_is_case_sensitive_explore_plan() {
    assert!(ONE_SHOT_BUILTIN_AGENT_TYPES.contains(&"Explore"));
    assert!(ONE_SHOT_BUILTIN_AGENT_TYPES.contains(&"Plan"));
    assert!(!ONE_SHOT_BUILTIN_AGENT_TYPES.contains(&"explore"));
    assert!(!ONE_SHOT_BUILTIN_AGENT_TYPES.contains(&"plan"));
    assert!(!ONE_SHOT_BUILTIN_AGENT_TYPES.contains(&"verification"));
    assert_eq!(ONE_SHOT_BUILTIN_AGENT_TYPES.len(), 2);
}

#[test]
fn empty_output_marker_matches_ts_literal() {
    assert_eq!(
        EMPTY_AGENT_OUTPUT_MARKER,
        "(Subagent completed but returned no output.)"
    );
}

// ── tools description ──

#[test]
fn tools_description_all_branches() {
    use coco_types::ToolAllowList;
    let wild = ToolAllowList::Wildcard;
    let explicit = |v: &[&str]| ToolAllowList::Explicit(v.iter().map(|s| (*s).into()).collect());

    assert_eq!(format_tools_description(&wild, &[]), "All tools");
    assert_eq!(
        format_tools_description(&wild, &["Bash".into(), "Edit".into()]),
        "All tools except Bash, Edit"
    );
    assert_eq!(
        format_tools_description(&explicit(&["Read", "Grep"]), &[]),
        "Read, Grep"
    );
    assert_eq!(
        format_tools_description(&explicit(&["Read", "Grep", "Bash"]), &["Bash".into()]),
        "Read, Grep"
    );
    assert_eq!(
        format_tools_description(&explicit(&["Bash"]), &["Bash".into()]),
        "None"
    );
    // Empty Explicit list renders as "All tools" — matches
    // `getToolsDescription`'s `allowedTools && allowedTools.length > 0`
    // gate (`prompt.ts:15-37`). The runtime filter (`filter.rs`) still
    // retains zero candidates for this state; rendering is purely
    // cosmetic.
    assert_eq!(
        format_tools_description(&ToolAllowList::Explicit(vec![]), &[]),
        "All tools"
    );
}

// ── allowed agent types parser ──

#[test]
fn parse_allowed_agent_types_handles_agent_and_task() {
    let from_agent = parse_allowed_agent_types("Agent(Explore, Plan)").unwrap();
    let from_task = parse_allowed_agent_types("Task(Explore,Plan)").unwrap();
    assert_eq!(from_agent.names, vec!["Explore", "Plan"]);
    assert_eq!(from_task.names, vec!["Explore", "Plan"]);
    assert!(from_agent.matches("Explore"));
    assert!(!from_agent.matches("explore"));
}

#[test]
fn parse_allowed_agent_types_ignores_unrelated_rules() {
    assert!(parse_allowed_agent_types("Bash(npm test)").is_none());
    assert!(parse_allowed_agent_types("Read").is_none());
}

#[test]
fn parse_allowed_agent_types_bare_agent_means_no_restriction() {
    // The regex captures group 2 as undefined for bare `Agent`; the runtime
    // treats undefined / empty as "no restriction". Returning None lets
    // callers skip the matching step entirely.
    assert!(parse_allowed_agent_types("Agent").is_none());
    assert!(parse_allowed_agent_types("Agent()").is_none());
    assert!(parse_allowed_agent_types("Task").is_none());
    assert!(parse_allowed_agent_types("Task()").is_none());
}

#[test]
fn allowed_agent_types_empty_names_means_match_all() {
    // If a future caller constructs AllowedAgentTypes with an empty list
    // explicitly, matches() must return true (no restriction).
    let unrestricted = filter::AllowedAgentTypes { names: vec![] };
    assert!(unrestricted.matches("Explore"));
    assert!(unrestricted.matches("anything"));
}

// ── async subagent tool clamp ──

#[test]
fn async_clamp_denies_non_async_safe_builtins_keeps_async_safe() {
    let denied = crate::async_subagent_disallowed_tools(/*plan_mode*/ false);
    // Long-lived / interactive tools are denied for background spawns.
    assert!(denied.contains(&ToolName::Repl.as_str()));
    assert!(denied.contains(&ToolName::Agent.as_str()));
    assert!(denied.contains(&ToolName::AskUserQuestion.as_str()));
    // Async-safe tools are NOT denied.
    assert!(!denied.contains(&ToolName::Read.as_str()));
    assert!(!denied.contains(&ToolName::Bash.as_str()));
    assert!(!denied.contains(&ToolName::Grep.as_str()));
    // ExitPlanMode is denied outside plan mode...
    assert!(denied.contains(&ToolName::ExitPlanMode.as_str()));
}

#[test]
fn async_clamp_readmits_exit_plan_mode_in_plan_mode() {
    let denied = crate::async_subagent_disallowed_tools(/*plan_mode*/ true);
    // ...but re-admitted in plan mode so a plan-mode background spawn can
    // still exit the plan.
    assert!(!denied.contains(&ToolName::ExitPlanMode.as_str()));
    // Other non-async-safe builtins stay denied.
    assert!(denied.contains(&ToolName::Repl.as_str()));
}

#[test]
fn frontmatter_wildcard_tools_collapses_to_default_allow_list() {
    // `parseAgentToolsFromFrontmatter` (`utils/markdownConfigLoader.ts:122-124`)
    // turns `tools: ['*']` into `undefined` (= use default allow set).
    // Coco-rs represents that with an empty allow-list.
    let project = TempDir::new().unwrap();
    write_md(
        project.path(),
        "wild.md",
        "---\nname: wild\ndescription: wildcard\ntools:\n  - '*'\n---\nbody",
    );
    let mut store = AgentDefinitionStore::new(
        BuiltinAgentCatalog::default(),
        AgentSearchPaths {
            project_dirs: vec![project.path().to_path_buf()],
            ..Default::default()
        },
    );
    store.load();
    let def = store.snapshot().find_active("wild").cloned().unwrap();
    assert!(
        def.allowed_tools.is_wildcard(),
        "got: {:?}",
        def.allowed_tools
    );
}

#[test]
fn frontmatter_description_unescapes_backslash_n() {
    // `loadAgentsDir.ts:565` does `.replace(/\\n/g, '\n')`.
    let project = TempDir::new().unwrap();
    write_md(
        project.path(),
        "multi.md",
        "---\nname: multi\ndescription: \"line1\\nline2\"\n---\nbody",
    );
    let mut store = AgentDefinitionStore::new(
        BuiltinAgentCatalog::default(),
        AgentSearchPaths {
            project_dirs: vec![project.path().to_path_buf()],
            ..Default::default()
        },
    );
    store.load();
    let def = store.snapshot().find_active("multi").cloned().unwrap();
    assert_eq!(def.when_to_use.as_deref(), Some("line1\nline2"));
}

#[test]
fn frontmatter_invalid_effort_warns_and_drops() {
    let project = TempDir::new().unwrap();
    write_md(
        project.path(),
        "bad.md",
        "---\nname: bad\ndescription: x\neffort: potato\n---\nbody",
    );
    let mut store = AgentDefinitionStore::new(
        BuiltinAgentCatalog::default(),
        AgentSearchPaths {
            project_dirs: vec![project.path().to_path_buf()],
            ..Default::default()
        },
    );
    store.load();
    let def = store.snapshot().find_active("bad").cloned().unwrap();
    assert!(def.effort.is_none());
    assert!(!store.last_report().warnings.is_empty());
}

#[test]
fn frontmatter_model_role_snake_case_parses() {
    let project = TempDir::new().unwrap();
    write_md(
        project.path(),
        "scout.md",
        "---\nname: scout\ndescription: roams\nmodel_role: explore\n---\nbody",
    );
    let mut store = AgentDefinitionStore::new(
        BuiltinAgentCatalog::default(),
        AgentSearchPaths {
            project_dirs: vec![project.path().to_path_buf()],
            ..Default::default()
        },
    );
    store.load();
    let def = store.snapshot().find_active("scout").cloned().unwrap();
    assert_eq!(def.model_role, Some(coco_types::ModelRole::Explore));
}

#[test]
fn frontmatter_model_role_camel_case_alias_parses() {
    let project = TempDir::new().unwrap();
    write_md(
        project.path(),
        "hooky.md",
        "---\nname: hooky\ndescription: hookish\nmodelRole: hookAgent\n---\nbody",
    );
    let mut store = AgentDefinitionStore::new(
        BuiltinAgentCatalog::default(),
        AgentSearchPaths {
            project_dirs: vec![project.path().to_path_buf()],
            ..Default::default()
        },
    );
    store.load();
    let def = store.snapshot().find_active("hooky").cloned().unwrap();
    assert_eq!(def.model_role, Some(coco_types::ModelRole::HookAgent));
}

#[test]
fn frontmatter_invalid_model_role_warns_and_drops() {
    let project = TempDir::new().unwrap();
    write_md(
        project.path(),
        "bad.md",
        "---\nname: bad\ndescription: x\nmodel_role: octopus\n---\nbody",
    );
    let mut store = AgentDefinitionStore::new(
        BuiltinAgentCatalog::default(),
        AgentSearchPaths {
            project_dirs: vec![project.path().to_path_buf()],
            ..Default::default()
        },
    );
    store.load();
    let def = store.snapshot().find_active("bad").cloned().unwrap();
    assert!(def.model_role.is_none());
    assert!(
        store
            .last_report()
            .warnings
            .iter()
            .any(|w| matches!(&w.error, ValidationError::InvalidModelRole { value } if value == "octopus")),
        "expected InvalidModelRole warning, got: {:?}",
        store.last_report().warnings
    );
}

#[test]
fn frontmatter_invalid_permission_mode_warns_and_drops() {
    let project = TempDir::new().unwrap();
    write_md(
        project.path(),
        "bad.md",
        "---\nname: bad\ndescription: x\npermissionMode: yolo\n---\nbody",
    );
    let mut store = AgentDefinitionStore::new(
        BuiltinAgentCatalog::default(),
        AgentSearchPaths {
            project_dirs: vec![project.path().to_path_buf()],
            ..Default::default()
        },
    );
    store.load();
    let def = store.snapshot().find_active("bad").cloned().unwrap();
    assert!(def.permission_mode.is_none());
    let warnings = &store.last_report().warnings;
    assert!(
        warnings.iter().any(|w| matches!(
            w.error,
            crate::validation::ValidationError::InvalidPermissionMode { .. }
        )),
        "got: {warnings:?}"
    );
}

#[test]
fn load_report_distinguishes_failures_from_warnings() {
    // A recoverable warning (invalid color dropped) should leave the
    // report failure-free; only true parse / validation failures count.
    let project = TempDir::new().unwrap();
    write_md(
        project.path(),
        "warn.md",
        "---\nname: warn\ndescription: x\ncolor: chartreuse\n---\nbody",
    );
    let mut store = AgentDefinitionStore::new(
        BuiltinAgentCatalog::default(),
        AgentSearchPaths {
            project_dirs: vec![project.path().to_path_buf()],
            ..Default::default()
        },
    );
    store.load();
    let report = store.last_report();
    assert!(!report.is_silent());
    assert!(!report.has_failures());
}

// ── definition store + prompt rendering ──

fn write_md(dir: &std::path::Path, name: &str, content: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

#[test]
fn store_loads_user_then_project_with_project_winning() {
    let user = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();

    write_md(
        user.path(),
        "build.md",
        "---\nname: build\ndescription: User build\nmodel: anthropic/claude-haiku-4-5\n---\nuser body",
    );
    write_md(
        project.path(),
        "build.md",
        "---\nname: build\ndescription: Project build\nmodel: anthropic/claude-sonnet-4-6\n---\nproject body",
    );

    let mut store = AgentDefinitionStore::new(
        BuiltinAgentCatalog::default(),
        AgentSearchPaths {
            user_dir: Some(user.path().to_path_buf()),
            project_dirs: vec![project.path().to_path_buf()],
            ..Default::default()
        },
    );
    store.load();
    let snap = store.snapshot();
    let active = snap.find_active("build").unwrap();
    assert_eq!(active.source, AgentSource::ProjectSettings);
    assert_eq!(active.model.as_deref(), Some("anthropic/claude-sonnet-4-6"));
    assert_eq!(active.system_prompt.as_deref(), Some("project body"));
}

#[test]
fn store_records_failed_files() {
    let project = TempDir::new().unwrap();
    write_md(
        project.path(),
        "broken.md",
        "---\ndescription: missing name\n---\nbody",
    );
    let mut store = AgentDefinitionStore::new(
        BuiltinAgentCatalog::default(),
        AgentSearchPaths {
            project_dirs: vec![project.path().to_path_buf()],
            ..Default::default()
        },
    );
    let report = store.load().clone();
    let snap = store.snapshot();
    assert!(snap.find_active("broken").is_none());
    assert_eq!(report.failed.len(), 1);
}

#[test]
fn store_records_color_warning_for_invalid_color() {
    // Invalid color is dropped and surfaces as a warning, while the
    // definition still loads and enters the active set.
    let project = TempDir::new().unwrap();
    write_md(
        project.path(),
        "fancy.md",
        "---\nname: fancy\ndescription: A colorful agent\ncolor: chartreuse\n---\nbody",
    );
    let mut store = AgentDefinitionStore::new(
        BuiltinAgentCatalog::default(),
        AgentSearchPaths {
            project_dirs: vec![project.path().to_path_buf()],
            ..Default::default()
        },
    );
    store.load();
    let snap = store.snapshot();
    assert!(snap.find_active("fancy").unwrap().color.is_none());
    let warnings = &store.last_report().warnings;
    assert_eq!(warnings.len(), 1);
    assert!(matches!(
        warnings[0].error,
        crate::validation::ValidationError::InvalidColor { .. }
    ));
}

#[test]
fn store_priority_chain_policy_overrides_everything() {
    let plugin = TempDir::new().unwrap();
    let user = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    let flag = TempDir::new().unwrap();
    let policy = TempDir::new().unwrap();
    for (dir, model_label) in [
        (&plugin, "plugin-model"),
        (&user, "user-model"),
        (&project, "project-model"),
        (&flag, "flag-model"),
        (&policy, "policy-model"),
    ] {
        write_md(
            dir.path(),
            "build.md",
            &format!("---\nname: build\ndescription: build agent\nmodel: {model_label}\n---\nbody"),
        );
    }
    let mut store = AgentDefinitionStore::new(
        BuiltinAgentCatalog::default(),
        AgentSearchPaths {
            user_dir: Some(user.path().to_path_buf()),
            project_dirs: vec![project.path().to_path_buf()],
            flag_dirs: vec![flag.path().to_path_buf()],
            policy_dirs: vec![policy.path().to_path_buf()],
            plugin_dirs: vec![PluginAgentDir {
                plugin_name: "myplugin".to_string(),
                dir: plugin.path().to_path_buf(),
            }],
        },
    );
    store.load();
    let active = store.snapshot();
    // The bare `build` name resolves to policy (highest source). The plugin's
    // `build` agent is namespaced `myplugin:build` and no longer collides.
    let build = active.find_active("build").unwrap();
    assert_eq!(build.source, AgentSource::PolicySettings);
    assert_eq!(build.model.as_deref(), Some("policy-model"));
    assert!(active.find_active("myplugin:build").is_some());
}

#[test]
fn store_plugin_agent_is_namespaced_and_security_gated() {
    // A plugin agent is registered as `<plugin>:<name>` and has the fields a
    // plugin is not trusted to declare (permissionMode / hooks / mcpServers)
    // stripped.
    let plugin = TempDir::new().unwrap();
    write_md(
        plugin.path(),
        "reviewer.md",
        "---\nname: reviewer\ndescription: review code\npermission-mode: bypassPermissions\n---\nbody",
    );
    let mut store = AgentDefinitionStore::new(
        BuiltinAgentCatalog::default(),
        AgentSearchPaths {
            plugin_dirs: vec![PluginAgentDir {
                plugin_name: "acme".to_string(),
                dir: plugin.path().to_path_buf(),
            }],
            ..Default::default()
        },
    );
    store.load();
    let snap = store.snapshot();
    // Bare name is NOT registered; the namespaced one is.
    assert!(snap.find_active("reviewer").is_none());
    let def = snap.find_active("acme:reviewer").expect("namespaced agent");
    assert_eq!(def.source, AgentSource::Plugin);
    assert_eq!(def.filename.as_deref(), Some("reviewer"));
    // Security gate: a plugin-declared permission_mode is stripped.
    assert_eq!(def.permission_mode, None);
}

#[test]
fn store_intra_dir_load_order_is_deterministic() {
    // Two same-name agents in one project dir resolve identically across
    // platforms (alphabetical filename wins because last-loaded wins).
    let project = TempDir::new().unwrap();
    write_md(
        project.path(),
        "a-build.md",
        "---\nname: build\ndescription: A\n---\nbody-a",
    );
    write_md(
        project.path(),
        "z-build.md",
        "---\nname: build\ndescription: Z\n---\nbody-z",
    );
    let mut store = AgentDefinitionStore::new(
        BuiltinAgentCatalog::default(),
        AgentSearchPaths {
            project_dirs: vec![project.path().to_path_buf()],
            ..Default::default()
        },
    );
    store.load();
    let snap = store.snapshot();
    let build = snap.find_active("build").unwrap();
    assert_eq!(build.system_prompt.as_deref(), Some("body-z"));
}

#[test]
fn frontmatter_tools_csv_string_is_split_on_commas() {
    let project = TempDir::new().unwrap();
    write_md(
        project.path(),
        "csv.md",
        "---\nname: csv\ndescription: CSV tools\ntools: Read, Edit, Write\n---\nbody",
    );
    let mut store = AgentDefinitionStore::new(
        BuiltinAgentCatalog::default(),
        AgentSearchPaths {
            project_dirs: vec![project.path().to_path_buf()],
            ..Default::default()
        },
    );
    store.load();
    let snap = store.snapshot();
    let def = snap.find_active("csv").unwrap();
    assert_eq!(
        def.allowed_tools,
        coco_types::ToolAllowList::Explicit(vec!["Read".into(), "Edit".into(), "Write".into()])
    );
}

#[test]
fn auto_memory_injection_adds_read_edit_write_when_enabled() {
    // `loadAgentsDir.ts:455-467` adds Read/Edit/Write to non-wildcard
    // tool lists when AutoMemory is on AND the agent declares a `memory`
    // scope. Coco-rs runs the same transform once the store's
    // `auto_memory_enabled` flag is set.
    let project = TempDir::new().unwrap();
    write_md(
        project.path(),
        "scribe.md",
        "---\n\
         name: scribe\n\
         description: writes to its own memory\n\
         tools:\n  - Bash\n  - Grep\n\
         memory: project\n\
         ---\n\
         body",
    );
    let mut store = AgentDefinitionStore::new(
        BuiltinAgentCatalog::default(),
        AgentSearchPaths {
            project_dirs: vec![project.path().to_path_buf()],
            ..Default::default()
        },
    );
    store.set_auto_memory_enabled(true);
    store.load();
    let snap = store.snapshot();
    let def = snap.find_active("scribe").unwrap();
    let mut tools = def
        .allowed_tools
        .as_explicit()
        .expect("scribe should have an Explicit allow-list after injection")
        .to_vec();
    tools.sort();
    assert_eq!(
        tools,
        vec![
            "Bash".to_owned(),
            "Edit".to_owned(),
            "Grep".to_owned(),
            "Read".to_owned(),
            "Write".to_owned(),
        ],
        "expected Bash + Grep (original) + Read/Edit/Write (injected)"
    );
}

#[test]
fn auto_memory_injection_skipped_for_wildcard_agents() {
    // Guards on `tools !== undefined`; coco-rs collapses `['*']` to
    // `ToolAllowList::Wildcard`. Either way the injection is a no-op
    // because the agent already sees every tool.
    let project = TempDir::new().unwrap();
    write_md(
        project.path(),
        "wild.md",
        "---\n\
         name: wild\n\
         description: wildcard\n\
         tools:\n  - '*'\n\
         memory: project\n\
         ---\n\
         body",
    );
    let mut store = AgentDefinitionStore::new(
        BuiltinAgentCatalog::default(),
        AgentSearchPaths {
            project_dirs: vec![project.path().to_path_buf()],
            ..Default::default()
        },
    );
    store.set_auto_memory_enabled(true);
    store.load();
    let snap = store.snapshot();
    let def = snap.find_active("wild").unwrap();
    assert!(
        def.allowed_tools.is_wildcard(),
        "wildcard agent's allow-list must remain Wildcard after injection, not {:?}",
        def.allowed_tools
    );
}

/// Regression: `tools: []` is **NOT** Wildcard. It means "zero tools"
/// (explicit empty list), and when paired with `memory:`, the auto-memory
/// injector promotes it to `[Read, Edit, Write]`.
///
/// `markdownConfigLoader.ts:113-126 parseAgentToolsFromFrontmatter`
/// returns `[]` for `tools: []`, then `loadAgentsDir.ts:455-456` gates
/// memory injection on `tools !== undefined` (true for `[]`).
///
/// Pre-fix Rust collapsed `tools: []` to `Wildcard`, silently broadening
/// such agents' tool pool to "all tools" and skipping memory injection
/// — opposite of user intent.
#[test]
fn auto_memory_injection_runs_for_explicit_empty_tools_with_memory() {
    let project = TempDir::new().unwrap();
    write_md(
        project.path(),
        "minimal.md",
        "---\n\
         name: minimal\n\
         description: zero-tools agent with memory\n\
         tools: []\n\
         memory: project\n\
         ---\n\
         body",
    );
    let mut store = AgentDefinitionStore::new(
        BuiltinAgentCatalog::default(),
        AgentSearchPaths {
            project_dirs: vec![project.path().to_path_buf()],
            ..Default::default()
        },
    );
    store.set_auto_memory_enabled(true);
    store.load();
    let snap = store.snapshot();
    let def = snap.find_active("minimal").unwrap();
    let mut tools = def
        .allowed_tools
        .as_explicit()
        .expect("`tools: []` with memory must produce Explicit (not Wildcard) after injection")
        .to_vec();
    tools.sort();
    assert_eq!(
        tools,
        vec!["Edit".to_owned(), "Read".to_owned(), "Write".to_owned()],
        "auto-memory must inject Read/Edit/Write into `tools: []`; \
         got {tools:?}",
    );
}

#[test]
fn auto_memory_injection_no_op_without_memory_scope() {
    // Memory scope unset → no injection even when AutoMemory is on.
    let project = TempDir::new().unwrap();
    write_md(
        project.path(),
        "plain.md",
        "---\n\
         name: plain\n\
         description: no memory declared\n\
         tools:\n  - Bash\n  - Grep\n\
         ---\n\
         body",
    );
    let mut store = AgentDefinitionStore::new(
        BuiltinAgentCatalog::default(),
        AgentSearchPaths {
            project_dirs: vec![project.path().to_path_buf()],
            ..Default::default()
        },
    );
    store.set_auto_memory_enabled(true);
    store.load();
    let snap = store.snapshot();
    let def = snap.find_active("plain").unwrap();
    assert_eq!(
        def.allowed_tools,
        coco_types::ToolAllowList::Explicit(vec!["Bash".into(), "Grep".into()])
    );
}

#[test]
fn auto_memory_injection_off_by_default() {
    // The flag defaults to false — pure-logic crate must not pre-suppose
    // a feature surface. Without `set_auto_memory_enabled(true)` the
    // tools list is unchanged.
    let project = TempDir::new().unwrap();
    write_md(
        project.path(),
        "scribe.md",
        "---\n\
         name: scribe\n\
         description: writes to its own memory\n\
         tools:\n  - Bash\n\
         memory: project\n\
         ---\n\
         body",
    );
    let mut store = AgentDefinitionStore::new(
        BuiltinAgentCatalog::default(),
        AgentSearchPaths {
            project_dirs: vec![project.path().to_path_buf()],
            ..Default::default()
        },
    );
    store.load();
    let snap = store.snapshot();
    let def = snap.find_active("scribe").unwrap();
    assert_eq!(
        def.allowed_tools,
        coco_types::ToolAllowList::Explicit(vec!["Bash".into()])
    );
}

#[test]
fn store_includes_builtins_alongside_custom() {
    let project = TempDir::new().unwrap();
    write_md(
        project.path(),
        "build.md",
        "---\nname: build\ndescription: Custom build agent\n---\nbody",
    );
    let mut store = AgentDefinitionStore::new(
        BuiltinAgentCatalog::all_enabled(),
        AgentSearchPaths {
            project_dirs: vec![project.path().to_path_buf()],
            ..Default::default()
        },
    );
    store.load();
    let snap = store.snapshot();
    assert!(snap.find_active("Explore").is_some());
    assert!(snap.find_active("build").is_some());
    assert!(snap.find_active("general-purpose").is_some());
}

#[test]
fn prompt_lists_active_agents_in_source_load_order() {
    // #117 / `getActiveAgentsFromList`: built-in agents render first
    // (in load order), then project agents — NOT alphabetical.
    let project = TempDir::new().unwrap();
    write_md(
        project.path(),
        "build.md",
        "---\nname: build\ndescription: Build verification\ntools:\n  - Bash\n  - Read\n---\nbody",
    );
    let mut store = AgentDefinitionStore::new(
        BuiltinAgentCatalog::default(),
        AgentSearchPaths {
            project_dirs: vec![project.path().to_path_buf()],
            ..Default::default()
        },
    );
    store.load();
    let snap = store.snapshot();
    let renderer = AgentToolPromptRenderer::new(&snap);
    let listing = renderer.agent_list(&PromptOptions::default());
    let lines: Vec<&str> = listing.lines().collect();
    assert_eq!(
        lines,
        vec![
            "- general-purpose: General-purpose agent for researching complex questions, searching for code, and executing multi-step tasks. When you are searching for a keyword or file and are not confident that you will find the right match in the first few tries use this agent to perform the search for you. (Tools: All tools)",
            "- statusline-setup: Use this agent to configure the user's Coco status line setting. (Tools: Read, Edit)",
            "- build: Build verification (Tools: Bash, Read)",
        ]
    );
}

#[test]
fn prompt_filters_by_allowed_agent_types() {
    let mut store = AgentDefinitionStore::new(
        BuiltinAgentCatalog::all_enabled(),
        AgentSearchPaths::empty(),
    );
    store.load();
    let snap = store.snapshot();
    let renderer = AgentToolPromptRenderer::new(&snap);
    let listing = renderer.agent_list(&PromptOptions {
        allowed_agent_types: Some(vec![SubagentType::Explore.as_str().to_owned()]),
        ..Default::default()
    });
    assert_eq!(listing.lines().count(), 1);
    assert!(listing.starts_with("- Explore: "));
}

#[test]
fn prompt_filters_by_denied_agent_types() {
    let mut store = AgentDefinitionStore::new(
        BuiltinAgentCatalog::all_enabled(),
        AgentSearchPaths::empty(),
    );
    store.load();
    let snap = store.snapshot();
    let renderer = AgentToolPromptRenderer::new(&snap);
    // Mirror TS `filterDeniedAgents`: an `Agent(general-purpose)` deny rule
    // removes that agent from the listing; non-denied agents remain.
    let listing = renderer.agent_list(&PromptOptions {
        denied_agent_types: vec!["general-purpose".to_owned()],
        ..Default::default()
    });
    assert!(
        !listing.contains("- general-purpose:"),
        "denied agent must be dropped from the listing: {listing}"
    );
    assert!(
        listing.contains("- statusline-setup:"),
        "non-denied agents must remain in the listing: {listing}"
    );
}
