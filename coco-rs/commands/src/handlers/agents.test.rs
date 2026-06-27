use super::*;
use coco_subagent::definition_store::AgentSearchPaths;
use coco_utils_common::COCO_CONFIG_DIR_NAME;
use std::path::PathBuf;

fn config_path(root: &str, child: &str) -> String {
    format!("{root}/{COCO_CONFIG_DIR_NAME}/{child}")
}

fn empty_paths() -> AgentSearchPaths {
    AgentSearchPaths {
        user_dir: None,
        project_dirs: Vec::<PathBuf>::new(),
        flag_dirs: Vec::<PathBuf>::new(),
        policy_dirs: Vec::<PathBuf>::new(),
        plugin_dirs: Vec::new(),
    }
}

#[test]
fn list_with_only_builtins_renders_count_and_each_agent() {
    let out = render("list", empty_paths()).unwrap();
    // Built-in catalog (interactive) ships at least general-purpose +
    // statusline-setup + Explore + Plan + coco-guide. Don't pin
    // exact count — just that the list is non-empty and well-formed.
    assert!(out.starts_with(|c: char| c.is_ascii_digit()), "got: {out}");
    assert!(out.contains("general-purpose"));
    assert!(out.contains("[built-in"));
}

#[test]
fn empty_subcommand_aliases_to_list() {
    let listed = render("list", empty_paths()).unwrap();
    let empty = render("", empty_paths()).unwrap();
    assert_eq!(listed, empty);
}

#[test]
fn show_unknown_agent_reports_not_found() {
    let out = render("show no-such-agent", empty_paths()).unwrap();
    assert!(out.contains("No active agent named"));
    assert!(out.contains("no-such-agent"));
}

#[test]
fn show_known_builtin_renders_metadata() {
    let out = render("show general-purpose", empty_paths()).unwrap();
    assert!(out.contains("# general-purpose"));
    assert!(out.contains("Source:"));
    assert!(out.contains("built-in"));
}

#[test]
fn show_without_name_returns_usage() {
    let out = render("show", empty_paths()).unwrap();
    assert!(out.contains("Usage: /agents show <name>"));
}

#[test]
fn paths_lists_built_in_first() {
    let out = render("paths", empty_paths()).unwrap();
    assert!(out.contains("built-in"));
}

#[test]
fn paths_includes_configured_dirs() {
    let user_agents = config_path("/home/u", "agents");
    let project_agents = config_path("/proj", "agents");
    let paths = AgentSearchPaths {
        user_dir: Some(PathBuf::from(&user_agents)),
        project_dirs: vec![PathBuf::from(&project_agents)],
        flag_dirs: Vec::<PathBuf>::new(),
        policy_dirs: Vec::<PathBuf>::new(),
        plugin_dirs: Vec::new(),
    };
    let out = render("paths", paths).unwrap();
    assert!(out.contains(&user_agents));
    assert!(out.contains(&project_agents));
}

#[test]
fn validate_with_no_diagnostics_reports_clean() {
    let out = render("validate", empty_paths()).unwrap();
    assert!(out.contains("loaded with no warnings"));
}

#[test]
fn unknown_subcommand_returns_usage_hint() {
    let out = render("explode", empty_paths()).unwrap();
    assert!(out.contains("Unknown /agents subcommand: explode"));
    assert!(out.contains("Usage:"));
}

#[test]
fn list_includes_navigation_hints() {
    // The overlay opens a 2-level menu (list → per-agent submenu). Flat-text
    // listing must surface the equivalent navigation paths so users
    // discover `/agents show <name>` and `/agents <name>` shorthand.
    let out = render("list", empty_paths()).unwrap();
    assert!(out.contains("/agents show <name>"));
    assert!(out.contains("/agents <name>"));
    assert!(out.contains("/agents reload"));
}

#[test]
fn name_shortcut_resolves_to_show() {
    // `/agents general-purpose` should match the picker click behavior
    // (selects an agent → opens its detail submenu). Bundled
    // `general-purpose` is stable across releases.
    let direct = render("general-purpose", empty_paths()).unwrap();
    let via_show = render("show general-purpose", empty_paths()).unwrap();
    assert_eq!(direct, via_show, "shortcut must equal `show <name>` output");
    assert!(direct.contains("# general-purpose"));
}

#[test]
fn reload_is_honest_about_session_scope() {
    // The handler can't push changes to the engine's live agent registry
    // (loaded once at startup), so the message must say so — pretending
    // otherwise would silently leave callers acting on stale state.
    let out = render("reload", empty_paths()).unwrap();
    assert!(
        out.contains("next session"),
        "reload output must call out the deferral, got: {out}"
    );
    // Still includes the live disk snapshot so users see what *will* load
    // next time.
    assert!(out.contains("general-purpose"));
}
