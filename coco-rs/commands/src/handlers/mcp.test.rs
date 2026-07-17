use super::*;
use coco_config::global_config::load_global_config_at;
use pretty_assertions::assert_eq;

/// A `/mcp` fixture rooted entirely in a tempdir — never the real `~/.cocode`
/// or the real `~/.cocode.json`.
struct Fixture {
    _tmp: tempfile::TempDir,
    paths: McpPaths,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let project_root = tmp.path().join("project");
        let config_home = tmp.path().join("config-home");
        std::fs::create_dir_all(&project_root).unwrap();
        std::fs::create_dir_all(&config_home).unwrap();
        Self {
            paths: McpPaths {
                project_root: project_root.clone(),
                session_cwd: project_root,
                config_home,
                global_config: tmp.path().join("global.json"),
            },
            _tmp: tmp,
        }
    }

    /// Write `{"mcpServers": {<name>: <config>}}` to a config file.
    fn write_config(&self, path: &Path, name: &str, config: serde_json::Value) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let doc = serde_json::json!({ "mcpServers": { name: config } });
        std::fs::write(path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
    }

    fn project_mcp_json(&self) -> PathBuf {
        self.paths.project_root.join(".mcp.json")
    }

    fn coco_mcp_json(&self) -> PathBuf {
        self.paths.add_target()
    }

    fn user_mcp_json(&self) -> PathBuf {
        self.paths.config_home.join("mcp.json")
    }

    fn managed_mcp_json(&self) -> PathBuf {
        self.paths.config_home.join("managed-mcp.json")
    }

    fn enterprise_mcp_json(&self) -> PathBuf {
        self.paths.config_home.join("enterprise-mcp.json")
    }

    /// What the session actually connects: loader output filtered by the same
    /// activation authority `/mcp list` renders — the assertion that matters.
    fn activates(&self, name: &str) -> bool {
        self.activates_with(name, &McpPolicyConfig::default())
    }

    fn activates_with(&self, name: &str, policy: &McpPolicyConfig) -> bool {
        let activation_policy = self.paths.activation_policy(policy);
        let servers = McpConfigLoader::load_with_roots(self.paths.roots(), &self.paths.config_home);
        activation_policy
            .filter_active(servers)
            .iter()
            .any(|server| server.name == name)
    }

    fn read(&self, path: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn project_entry(&self) -> coco_config::global_config::ProjectConfig {
        load_global_config_at(&self.paths.global_config)
            .unwrap()
            .projects
            .get(&coco_mcp::project_key(&self.paths.project_root))
            .cloned()
            .unwrap_or_default()
    }
}

fn stdio(command: &str) -> serde_json::Value {
    serde_json::json!({ "command": command, "args": ["--stdio"] })
}

fn no_policy() -> McpPolicyConfig {
    McpPolicyConfig::default()
}

/// Regression: the loader reads `.mcp.json`, but `/mcp list` used to read
/// settings.json and miss it entirely.
#[tokio::test]
async fn test_list_shows_server_defined_in_mcp_json() {
    let fixture = Fixture::new();
    fixture.write_config(&fixture.user_mcp_json(), "filesystem", stdio("npx"));

    let output = list_mcp_servers(&fixture.paths, &no_policy())
        .await
        .unwrap();

    assert!(output.contains("filesystem"), "{output}");
    assert!(output.contains("active"), "{output}");
    assert!(output.contains("stdio"), "{output}");
    assert!(output.contains("user"), "{output}");
    assert!(output.contains("mcp.json"), "{output}");
}

/// Project-scope servers arrive with the repository: they fail closed until
/// approved, and `/mcp list` says so.
#[tokio::test]
async fn test_list_shows_project_server_awaiting_approval() {
    let fixture = Fixture::new();
    fixture.write_config(&fixture.project_mcp_json(), "repo-srv", stdio("npx"));

    let output = list_mcp_servers(&fixture.paths, &no_policy())
        .await
        .unwrap();

    assert!(!fixture.activates("repo-srv"), "must not auto-connect");
    assert!(output.contains("needs approval"), "{output}");
    assert!(output.contains("/mcp enable"), "{output}");
}

/// Trusted-source `enable_all_project_mcp_servers` pre-approves repo servers.
#[tokio::test]
async fn test_list_project_server_active_when_pre_approved() {
    let fixture = Fixture::new();
    fixture.write_config(&fixture.project_mcp_json(), "repo-srv", stdio("npx"));
    let policy = McpPolicyConfig {
        project_servers_pre_approved: true,
        ..Default::default()
    };

    let output = list_mcp_servers(&fixture.paths, &policy).await.unwrap();

    assert!(fixture.activates_with("repo-srv", &policy));
    assert!(output.contains("active"), "{output}");
}

/// A residual legacy `"disabled": true` keeps the entry off (fail-safe) and
/// the list explains the migration.
#[tokio::test]
async fn test_list_shows_legacy_disabled_entry_and_loader_refuses_it() {
    let fixture = Fixture::new();
    fixture.write_config(
        &fixture.user_mcp_json(),
        "db",
        serde_json::json!({ "command": "node", "disabled": true }),
    );

    let output = list_mcp_servers(&fixture.paths, &no_policy())
        .await
        .unwrap();

    assert!(!fixture.activates("db"), "legacy entry must stay off");
    assert!(output.contains("db"), "{output}");
    assert!(output.contains("disabled*"), "{output}");
    assert!(output.contains("removed \"disabled\" field"), "{output}");
    // Transport still resolves for a legacy-disabled entry.
    assert!(output.contains("stdio"), "{output}");
}

/// The tombstone the old skip-semantics never had: a legacy `disabled` entry
/// in a later-loading file now *wins the merge* and fail-safes off, instead
/// of the earlier enabled definition silently surviving.
#[tokio::test]
async fn test_legacy_disabled_in_later_layer_masks_earlier_definition() {
    let fixture = Fixture::new();
    fixture.write_config(&fixture.user_mcp_json(), "srv", stdio("user-cmd"));
    fixture.write_config(
        &fixture.managed_mcp_json(),
        "srv",
        serde_json::json!({ "command": "user-cmd", "disabled": true }),
    );

    assert!(
        !fixture.activates("srv"),
        "the managed layer's entry wins the merge; fail-safe keeps it off"
    );
}

#[tokio::test]
async fn test_list_denied_server_shows_denied() {
    let fixture = Fixture::new();
    fixture.write_config(&fixture.user_mcp_json(), "banned", stdio("npx"));
    let policy = McpPolicyConfig {
        denied_servers: vec![coco_config::DeniedMcpServerEntry {
            name: "banned".to_string(),
            command: None,
            url: None,
        }],
        ..Default::default()
    };

    let output = list_mcp_servers(&fixture.paths, &policy).await.unwrap();

    assert!(!fixture.activates_with("banned", &policy));
    assert!(output.contains("denied"), "{output}");
}

#[tokio::test]
async fn test_list_reports_scope_for_each_source_file() {
    let fixture = Fixture::new();
    fixture.write_config(&fixture.user_mcp_json(), "user-server", stdio("uvx"));
    fixture.write_config(&fixture.coco_mcp_json(), "project-server", stdio("npx"));

    let output = list_mcp_servers(&fixture.paths, &no_policy())
        .await
        .unwrap();

    assert!(output.contains("user-server"), "{output}");
    assert!(output.contains("user"), "{output}");
    assert!(output.contains("project-server"), "{output}");
    assert!(output.contains("2 servers configured"), "{output}");
}

#[tokio::test]
async fn test_list_empty_state_names_real_loader_files() {
    let fixture = Fixture::new();

    let output = list_mcp_servers(&fixture.paths, &no_policy())
        .await
        .unwrap();

    assert!(output.contains("No MCP servers configured"), "{output}");
    assert!(output.contains(".mcp.json"), "{output}");
    assert!(output.contains("mcp.json"), "{output}");
    assert!(output.contains("mcpServers"), "{output}");
    // The old help text pointed at settings.json, which the loader never reads.
    assert!(!output.contains("settings.json"), "{output}");
}

/// The empty-state example must be a config the loader actually parses.
#[tokio::test]
async fn test_list_empty_state_example_is_loadable() {
    let fixture = Fixture::new();
    let example = serde_json::json!({
        "command": "npx",
        "args": ["-y", "@modelcontextprotocol/server-filesystem"],
    });
    assert!(coco_mcp::config::parse_server_config(&example).is_some());

    fixture.write_config(&fixture.user_mcp_json(), "my-server", example);
    assert!(fixture.activates("my-server"));
}

/// The core fix: a disable is a user-side toggle, not a definition edit — no
/// mcp.json file changes, and the session stops connecting the server.
#[tokio::test]
async fn test_disable_writes_user_toggle_not_definition_file() {
    let fixture = Fixture::new();
    fixture.write_config(&fixture.user_mcp_json(), "filesystem", stdio("npx"));
    assert!(fixture.activates("filesystem"), "precondition: activates");
    let definition_before = fixture.read(&fixture.user_mcp_json());

    let output = disable_server("filesystem", &fixture.paths).await.unwrap();

    assert!(output.contains("Disabled"), "{output}");
    assert_eq!(
        fixture.read(&fixture.user_mcp_json()),
        definition_before,
        "definition files must not change"
    );
    assert!(
        fixture
            .project_entry()
            .disabled_mcp_servers
            .contains("filesystem"),
        "toggle lands in GlobalConfig.projects"
    );
    assert!(!fixture.activates("filesystem"), "must stop activating");
}

/// The old file-edit disable leaked when a lower-precedence file defined the
/// same name. The name-keyed toggle cannot leak: whatever definition wins the
/// merge, the toggle switches it off.
#[tokio::test]
async fn test_disable_holds_across_duplicate_definitions() {
    let fixture = Fixture::new();
    fixture.write_config(&fixture.user_mcp_json(), "dup", stdio("uvx"));
    fixture.write_config(&fixture.coco_mcp_json(), "dup", stdio("npx"));

    disable_server("dup", &fixture.paths).await.unwrap();

    assert!(
        !fixture.activates("dup"),
        "no duplicate definition may survive a disable"
    );
}

#[tokio::test]
async fn test_enable_clears_toggle_and_approves_project_server() {
    let fixture = Fixture::new();
    fixture.write_config(&fixture.project_mcp_json(), "repo-srv", stdio("npx"));
    assert!(!fixture.activates("repo-srv"), "precondition: gated");

    let output = enable_server("repo-srv", &fixture.paths, &no_policy())
        .await
        .unwrap();

    assert!(output.contains("Enabled"), "{output}");
    assert!(output.contains("approved"), "{output}");
    assert!(
        fixture
            .project_entry()
            .approved_mcp_servers
            .contains("repo-srv"),
        "approval lands in GlobalConfig.projects"
    );
    assert!(fixture.activates("repo-srv"), "approved server activates");
}

#[tokio::test]
async fn test_enable_migrates_legacy_disabled_field() {
    let fixture = Fixture::new();
    fixture.write_config(
        &fixture.user_mcp_json(),
        "filesystem",
        serde_json::json!({ "command": "npx", "disabled": true }),
    );
    assert!(
        !fixture.activates("filesystem"),
        "precondition: fail-safe off"
    );

    let output = enable_server("filesystem", &fixture.paths, &no_policy())
        .await
        .unwrap();

    assert!(output.contains("Enabled"), "{output}");
    let written = fixture.read(&fixture.user_mcp_json());
    assert_eq!(
        written["mcpServers"]["filesystem"].get("disabled"),
        None,
        "the legacy field is migrated out on explicit enable"
    );
    assert!(fixture.activates("filesystem"));
}

#[tokio::test]
async fn test_enable_refuses_denied_server() {
    let fixture = Fixture::new();
    fixture.write_config(&fixture.user_mcp_json(), "banned", stdio("npx"));
    let policy = McpPolicyConfig {
        denied_servers: vec![coco_config::DeniedMcpServerEntry {
            name: "banned".to_string(),
            command: None,
            url: None,
        }],
        ..Default::default()
    };

    let output = enable_server("banned", &fixture.paths, &policy)
        .await
        .unwrap();

    assert!(output.contains("Cannot enable 'banned'"), "{output}");
    assert!(!fixture.activates_with("banned", &policy));
}

/// The user toggle must not switch off an admin-mandated policy server.
#[tokio::test]
async fn test_disable_managed_scope_is_refused() {
    let fixture = Fixture::new();
    fixture.write_config(&fixture.managed_mcp_json(), "policy-server", stdio("npx"));

    let output = disable_server("policy-server", &fixture.paths)
        .await
        .unwrap();

    assert!(output.contains("Cannot modify 'policy-server'"), "{output}");
    assert!(output.contains("managed"), "{output}");
    assert!(
        fixture.activates("policy-server"),
        "policy server still runs"
    );
}

#[tokio::test]
async fn test_remove_enterprise_scope_is_refused_not_edited() {
    let fixture = Fixture::new();
    fixture.write_config(&fixture.enterprise_mcp_json(), "corp", stdio("npx"));
    let before = fixture.read(&fixture.enterprise_mcp_json());

    let output = remove_server("corp", &fixture.paths).await.unwrap();

    assert!(output.contains("Cannot modify 'corp'"), "{output}");
    assert!(output.contains("enterprise"), "{output}");
    assert_eq!(fixture.read(&fixture.enterprise_mcp_json()), before);
}

/// A policy file wins the merge over a project definition, so the toggle
/// refusal applies to the policy scope — the project file is left alone too.
#[tokio::test]
async fn test_policy_definition_wins_over_project_and_is_refused() {
    let fixture = Fixture::new();
    fixture.write_config(&fixture.project_mcp_json(), "shared", stdio("npx"));
    fixture.write_config(&fixture.managed_mcp_json(), "shared", stdio("policy-cmd"));
    let project_before = fixture.read(&fixture.project_mcp_json());

    let output = disable_server("shared", &fixture.paths).await.unwrap();

    assert!(output.contains("Cannot modify 'shared'"), "{output}");
    assert_eq!(fixture.read(&fixture.project_mcp_json()), project_before);
}

#[tokio::test]
async fn test_add_writes_project_scoped_mcp_json_the_loader_reads() {
    let fixture = Fixture::new();

    let output = add_server("my-server npx -y some-package", &fixture.paths)
        .await
        .unwrap();

    assert!(output.contains("Added MCP server 'my-server'"), "{output}");
    assert!(
        output.contains("mcp.json"),
        "message names the file: {output}"
    );
    let written = fixture.read(&fixture.coco_mcp_json());
    assert_eq!(written["mcpServers"]["my-server"]["command"], "npx");
    assert_eq!(
        written["mcpServers"]["my-server"]["args"],
        serde_json::json!(["-y", "some-package"])
    );
    // settings.json is not part of the loader's file set.
    assert!(
        !fixture
            .paths
            .project_root
            .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
            .join("settings.json")
            .exists()
    );
}

#[tokio::test]
async fn test_add_preserves_existing_servers_in_file() {
    let fixture = Fixture::new();
    fixture.write_config(&fixture.coco_mcp_json(), "existing", stdio("uvx"));

    add_server("added npx pkg", &fixture.paths).await.unwrap();

    let written = fixture.read(&fixture.coco_mcp_json());
    assert_eq!(written["mcpServers"]["existing"]["command"], "uvx");
    assert_eq!(written["mcpServers"]["added"]["command"], "npx");
}

#[tokio::test]
async fn test_add_warns_when_higher_precedence_file_shadows_name() {
    let fixture = Fixture::new();
    fixture.write_config(&fixture.managed_mcp_json(), "shadowed", stdio("policy-cmd"));

    let output = add_server("shadowed npx pkg", &fixture.paths)
        .await
        .unwrap();

    assert!(output.contains("loads later and wins"), "{output}");
    assert!(output.contains("managed"), "{output}");
}

#[tokio::test]
async fn test_remove_deletes_from_defining_file() {
    let fixture = Fixture::new();
    fixture.write_config(&fixture.user_mcp_json(), "gone", stdio("npx"));

    let output = remove_server("gone", &fixture.paths).await.unwrap();

    assert!(output.contains("Removed MCP server 'gone'"), "{output}");
    assert!(output.contains("mcp.json"), "{output}");
    let written = fixture.read(&fixture.user_mcp_json());
    assert_eq!(written["mcpServers"].get("gone"), None);
    assert!(!fixture.activates("gone"));
}

#[tokio::test]
async fn test_remove_notes_surviving_lower_precedence_definition() {
    let fixture = Fixture::new();
    fixture.write_config(&fixture.user_mcp_json(), "dup", stdio("uvx"));
    fixture.write_config(&fixture.coco_mcp_json(), "dup", stdio("npx"));

    let output = remove_server("dup", &fixture.paths).await.unwrap();

    assert!(output.contains("also defined at user scope"), "{output}");
    assert!(output.contains("/mcp disable"), "{output}");
}

#[tokio::test]
async fn test_toggle_unknown_server_lists_searched_paths() {
    let fixture = Fixture::new();

    let output = disable_server("ghost", &fixture.paths).await.unwrap();

    assert!(output.contains("MCP server 'ghost' not found"), "{output}");
    assert!(output.contains(".mcp.json"), "{output}");
    assert!(output.contains("managed-mcp.json"), "{output}");
}

#[tokio::test]
async fn test_add_usage_when_command_missing() {
    let fixture = Fixture::new();

    let output = add_server("only-a-name", &fixture.paths).await.unwrap();

    assert!(output.contains("Usage: /mcp add"), "{output}");
    assert!(!fixture.coco_mcp_json().exists());
}

/// `run` must resolve config against the context roots it is handed, not the
/// process cwd: one app-server process hosts sessions from different projects.
#[tokio::test]
async fn test_run_resolves_config_against_supplied_roots() {
    let fixture = Fixture::new();
    fixture.write_config(&fixture.project_mcp_json(), "scoped-server", stdio("npx"));

    let context = McpCommandContext::for_cwd(fixture.paths.project_root.clone());
    let output = run("list", &context).await.unwrap();

    assert!(output.contains("scoped-server"), "{output}");
    // A different project root sees none of it.
    let elsewhere = tempfile::tempdir().unwrap();
    let other_context = McpCommandContext::for_cwd(elsewhere.path().to_path_buf());
    let other = run("list", &other_context).await.unwrap();
    assert!(!other.contains("scoped-server"), "{other}");
}

#[tokio::test]
async fn test_run_unknown_subcommand() {
    let fixture = Fixture::new();

    let context = McpCommandContext::for_cwd(fixture.paths.project_root.clone());
    let output = run("foobar", &context).await.unwrap();

    assert!(output.contains("Unknown MCP subcommand"));
    assert!(output.contains("Usage"));
}
