use super::*;
use pretty_assertions::assert_eq;

/// A `/mcp` fixture rooted entirely in a tempdir — never the real `~/.cocode`.
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
            _tmp: tmp,
            paths: McpPaths {
                project_root: project_root.clone(),
                session_cwd: project_root,
                config_home,
            },
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

    /// What the real loader would load — the assertion that matters.
    fn loads(&self, name: &str) -> bool {
        McpConfigLoader::load_with_roots(self.paths.roots(), &self.paths.config_home)
            .iter()
            .any(|server| server.name == name)
    }

    fn read(&self, path: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }
}

fn stdio(command: &str) -> serde_json::Value {
    serde_json::json!({ "command": command, "args": ["--stdio"] })
}

/// Regression: the loader reads `.mcp.json`, but `/mcp list` used to read
/// settings.json and miss it entirely.
#[tokio::test]
async fn test_list_shows_server_defined_in_mcp_json() {
    let fixture = Fixture::new();
    fixture.write_config(&fixture.project_mcp_json(), "filesystem", stdio("npx"));

    let output = list_mcp_servers(&fixture.paths).await.unwrap();

    assert!(output.contains("filesystem"), "{output}");
    assert!(output.contains("active"), "{output}");
    assert!(output.contains("stdio"), "{output}");
    assert!(output.contains("project"), "{output}");
    assert!(output.contains(".mcp.json"), "{output}");
}

#[tokio::test]
async fn test_list_shows_disabled_server_the_loader_skips() {
    let fixture = Fixture::new();
    fixture.write_config(
        &fixture.project_mcp_json(),
        "db",
        serde_json::json!({ "command": "node", "disabled": true }),
    );

    let output = list_mcp_servers(&fixture.paths).await.unwrap();

    assert!(!fixture.loads("db"), "loader must skip a disabled entry");
    assert!(output.contains("db"), "{output}");
    assert!(output.contains("disabled"), "{output}");
    // Transport still resolves for a disabled entry.
    assert!(output.contains("stdio"), "{output}");
}

#[tokio::test]
async fn test_list_reports_scope_for_each_source_file() {
    let fixture = Fixture::new();
    fixture.write_config(&fixture.user_mcp_json(), "user-server", stdio("uvx"));
    fixture.write_config(&fixture.coco_mcp_json(), "project-server", stdio("npx"));

    let output = list_mcp_servers(&fixture.paths).await.unwrap();

    assert!(output.contains("user-server"), "{output}");
    assert!(output.contains("user"), "{output}");
    assert!(output.contains("project-server"), "{output}");
    assert!(output.contains("2 servers configured"), "{output}");
}

#[tokio::test]
async fn test_list_empty_state_names_real_loader_files() {
    let fixture = Fixture::new();

    let output = list_mcp_servers(&fixture.paths).await.unwrap();

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

    fixture.write_config(&fixture.coco_mcp_json(), "my-server", example);
    assert!(fixture.loads("my-server"));
}

/// The trap: a disable must land in the defining file, and the loader must then
/// actually stop loading the server. Asserted end-to-end through the loader.
#[tokio::test]
async fn test_disable_edits_defining_file_and_loader_drops_server() {
    let fixture = Fixture::new();
    fixture.write_config(&fixture.project_mcp_json(), "filesystem", stdio("npx"));
    assert!(fixture.loads("filesystem"), "precondition: server loads");

    let output = toggle_server("filesystem", /*enable*/ false, &fixture.paths)
        .await
        .unwrap();

    assert!(output.contains("Disabled"), "{output}");
    assert!(
        output.contains(".mcp.json"),
        "message names the file: {output}"
    );
    let written = fixture.read(&fixture.project_mcp_json());
    assert_eq!(written["mcpServers"]["filesystem"]["disabled"], true);
    assert!(!fixture.loads("filesystem"), "loader must drop it");
}

/// Writing `disabled` into a different file would be a no-op: the loader skips
/// the disabled entry and keeps the earlier definition.
#[tokio::test]
async fn test_disable_does_not_write_shadowing_entry_elsewhere() {
    let fixture = Fixture::new();
    fixture.write_config(&fixture.project_mcp_json(), "filesystem", stdio("npx"));

    toggle_server("filesystem", /*enable*/ false, &fixture.paths)
        .await
        .unwrap();

    assert!(
        !fixture.coco_mcp_json().exists(),
        "must not create a shadowing .cocode/mcp.json"
    );
    assert!(
        !fixture.user_mcp_json().exists(),
        "must not create a shadowing user mcp.json"
    );
}

#[tokio::test]
async fn test_disable_targets_effective_definition_not_lower_scope() {
    let fixture = Fixture::new();
    // Both define "dup"; the project file loads later and wins.
    fixture.write_config(&fixture.user_mcp_json(), "dup", stdio("uvx"));
    fixture.write_config(&fixture.coco_mcp_json(), "dup", stdio("npx"));

    let output = toggle_server("dup", /*enable*/ false, &fixture.paths)
        .await
        .unwrap();

    // The winning (project) definition is the one edited.
    let project = fixture.read(&fixture.coco_mcp_json());
    assert_eq!(project["mcpServers"]["dup"]["disabled"], true);
    let user = fixture.read(&fixture.user_mcp_json());
    assert_eq!(user["mcpServers"]["dup"].get("disabled"), None);
    // The user definition survives, so the server still loads — say so.
    assert!(fixture.loads("dup"));
    assert!(output.contains("also defined at user scope"), "{output}");
}

#[tokio::test]
async fn test_enable_removes_disabled_from_defining_file() {
    let fixture = Fixture::new();
    fixture.write_config(
        &fixture.project_mcp_json(),
        "filesystem",
        serde_json::json!({ "command": "npx", "disabled": true }),
    );
    assert!(!fixture.loads("filesystem"), "precondition: disabled");

    let output = toggle_server("filesystem", /*enable*/ true, &fixture.paths)
        .await
        .unwrap();

    assert!(output.contains("Enabled"), "{output}");
    let written = fixture.read(&fixture.project_mcp_json());
    assert_eq!(written["mcpServers"]["filesystem"].get("disabled"), None);
    assert!(fixture.loads("filesystem"), "loader must pick it back up");
}

#[tokio::test]
async fn test_disable_managed_scope_is_refused_not_edited() {
    let fixture = Fixture::new();
    fixture.write_config(&fixture.managed_mcp_json(), "policy-server", stdio("npx"));
    let before = fixture.read(&fixture.managed_mcp_json());

    let output = toggle_server("policy-server", /*enable*/ false, &fixture.paths)
        .await
        .unwrap();

    assert!(output.contains("Cannot modify 'policy-server'"), "{output}");
    assert!(output.contains("managed"), "{output}");
    assert_eq!(fixture.read(&fixture.managed_mcp_json()), before);
    assert!(fixture.loads("policy-server"), "policy server still loads");
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

/// A policy file wins over a project definition, so the policy scope — not the
/// editable project file — is what an edit resolves to.
#[tokio::test]
async fn test_policy_definition_wins_over_project_and_is_refused() {
    let fixture = Fixture::new();
    fixture.write_config(&fixture.project_mcp_json(), "shared", stdio("npx"));
    fixture.write_config(&fixture.managed_mcp_json(), "shared", stdio("policy-cmd"));
    let project_before = fixture.read(&fixture.project_mcp_json());

    let output = toggle_server("shared", /*enable*/ false, &fixture.paths)
        .await
        .unwrap();

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
    assert!(
        fixture.loads("my-server"),
        "added server must actually load"
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
    fixture.write_config(&fixture.project_mcp_json(), "gone", stdio("npx"));

    let output = remove_server("gone", &fixture.paths).await.unwrap();

    assert!(output.contains("Removed MCP server 'gone'"), "{output}");
    assert!(output.contains(".mcp.json"), "{output}");
    let written = fixture.read(&fixture.project_mcp_json());
    assert_eq!(written["mcpServers"].get("gone"), None);
    assert!(!fixture.loads("gone"));
}

#[tokio::test]
async fn test_toggle_unknown_server_lists_searched_paths() {
    let fixture = Fixture::new();

    let output = toggle_server("ghost", /*enable*/ false, &fixture.paths)
        .await
        .unwrap();

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

#[tokio::test]
async fn test_run_unknown_subcommand() {
    let fixture = Fixture::new();

    let output = run("foobar", &fixture.paths.project_root).await.unwrap();

    assert!(output.contains("Unknown MCP subcommand"));
    assert!(output.contains("Usage"));
}

/// `run` must resolve config against the cwd it is handed, not the process cwd:
/// one app-server process hosts sessions from different projects.
#[tokio::test]
async fn test_run_resolves_config_against_supplied_cwd() {
    let fixture = Fixture::new();
    fixture.write_config(&fixture.project_mcp_json(), "scoped-server", stdio("npx"));

    let output = run("list", &fixture.paths.project_root).await.unwrap();

    assert!(output.contains("scoped-server"), "{output}");
    // A different project root sees none of it.
    let elsewhere = tempfile::tempdir().unwrap();
    let other = run("list", elsewhere.path()).await.unwrap();
    assert!(!other.contains("scoped-server"), "{other}");
}
