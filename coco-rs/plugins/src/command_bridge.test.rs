use std::collections::HashMap;

use coco_types::CommandSource;
use pretty_assertions::assert_eq;

use crate::loader::LoadedPluginV2;
use crate::loader::PluginLoadSource;
use crate::schemas::CommandMetadata;
use crate::schemas::ManifestCommands;
use crate::schemas::PluginId;
use crate::schemas::PluginManifestV2;

use super::*;

/// Build a minimal inline `LoadedPluginV2` (no manifest commands) for the
/// `commands/`-directory scanning tests.
fn test_plugin(name: &str, path: &std::path::Path) -> LoadedPluginV2 {
    LoadedPluginV2 {
        id: PluginId {
            name: name.to_string(),
            marketplace: "inline".to_string(),
        },
        manifest: PluginManifestV2 {
            name: name.to_string(),
            ..default_manifest_v2()
        },
        path: path.to_path_buf(),
        load_source: PluginLoadSource::SessionDir,
        enabled: true,
    }
}

#[test]
fn test_load_commands_from_md_files() {
    let dir = tempfile::tempdir().unwrap();
    let commands_dir = dir.path().join("commands");
    std::fs::create_dir(&commands_dir).unwrap();
    std::fs::write(
        commands_dir.join("deploy.md"),
        "---\ndescription: Deploy the app\n---\nRun deployment steps.\n",
    )
    .unwrap();

    let plugin = test_plugin("my-plugin", dir.path());
    let commands = load_plugin_commands_v2(&plugin);

    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].base.name, "my-plugin:deploy");
    assert_eq!(commands[0].base.description, "Deploy the app");
    assert_eq!(
        commands[0].base.loaded_from,
        Some(CommandSource::Plugin {
            name: "my-plugin".to_string()
        })
    );
    assert_eq!(commands[0].prompt, "Run deployment steps.");
}

#[test]
fn test_load_commands_from_skill_md_subdir() {
    let dir = tempfile::tempdir().unwrap();
    let commands_dir = dir.path().join("commands");
    let subdir = commands_dir.join("lint");
    std::fs::create_dir_all(&subdir).unwrap();
    std::fs::write(
        subdir.join("SKILL.md"),
        "---\ndescription: Lint code\n---\nRun linter.\n",
    )
    .unwrap();

    let plugin = test_plugin("code-tools", dir.path());
    let commands = load_plugin_commands_v2(&plugin);

    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].base.name, "code-tools:lint");
    assert_eq!(commands[0].base.description, "Lint code");
}

#[test]
fn test_load_commands_empty_plugin() {
    let dir = tempfile::tempdir().unwrap();
    let plugin = test_plugin("empty", dir.path());
    let commands = load_plugin_commands_v2(&plugin);
    assert!(commands.is_empty());
}

#[test]
fn test_load_all_plugin_commands_multiple() {
    let dir1 = tempfile::tempdir().unwrap();
    let dir2 = tempfile::tempdir().unwrap();

    let cmd_dir1 = dir1.path().join("commands");
    std::fs::create_dir(&cmd_dir1).unwrap();
    std::fs::write(cmd_dir1.join("a.md"), "# a\n\nCommand A.\n").unwrap();

    let cmd_dir2 = dir2.path().join("commands");
    std::fs::create_dir(&cmd_dir2).unwrap();
    std::fs::write(cmd_dir2.join("b.md"), "# b\n\nCommand B.\n").unwrap();

    let p1 = test_plugin("plugin1", dir1.path());
    let p2 = test_plugin("plugin2", dir2.path());

    let commands = load_all_plugin_commands_v2(&[&p1, &p2]);
    assert_eq!(commands.len(), 2);
    let names: Vec<&str> = commands.iter().map(|c| c.base.name.as_str()).collect();
    assert!(names.contains(&"plugin1:a"));
    assert!(names.contains(&"plugin2:b"));
}

#[test]
fn test_load_v2_commands_from_manifest_string_path() {
    let dir = tempfile::tempdir().unwrap();

    // Create command file at a custom path
    std::fs::write(
        dir.path().join("my-cmd.md"),
        "---\ndescription: Custom command\n---\nDo custom things.\n",
    )
    .unwrap();

    let plugin = LoadedPluginV2 {
        id: crate::schemas::PluginId {
            name: "test-v2".to_string(),
            marketplace: "inline".to_string(),
        },
        manifest: crate::schemas::PluginManifestV2 {
            name: "test-v2".to_string(),
            commands: Some(ManifestCommands::SinglePath("my-cmd.md".to_string())),
            ..default_manifest_v2()
        },
        path: dir.path().to_path_buf(),
        load_source: crate::loader::PluginLoadSource::SessionDir,
        enabled: true,
    };

    let commands = load_plugin_commands_v2(&plugin);
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].base.name, "test-v2:my-cmd");
}

#[test]
fn test_load_v2_commands_from_manifest_array() {
    let dir = tempfile::tempdir().unwrap();

    std::fs::write(dir.path().join("a.md"), "# a\n\nCommand A.\n").unwrap();
    std::fs::write(dir.path().join("b.md"), "# b\n\nCommand B.\n").unwrap();

    let plugin = LoadedPluginV2 {
        id: crate::schemas::PluginId {
            name: "arr-plugin".to_string(),
            marketplace: "inline".to_string(),
        },
        manifest: crate::schemas::PluginManifestV2 {
            name: "arr-plugin".to_string(),
            commands: Some(ManifestCommands::MultiplePaths(vec![
                "a.md".to_string(),
                "b.md".to_string(),
            ])),
            ..default_manifest_v2()
        },
        path: dir.path().to_path_buf(),
        load_source: crate::loader::PluginLoadSource::SessionDir,
        enabled: true,
    };

    let commands = load_plugin_commands_v2(&plugin);
    assert_eq!(commands.len(), 2);
    let names: Vec<&str> = commands.iter().map(|c| c.base.name.as_str()).collect();
    assert!(names.contains(&"arr-plugin:a"));
    assert!(names.contains(&"arr-plugin:b"));
}

#[test]
fn test_load_v2_commands_from_manifest_object_inline_content() {
    let dir = tempfile::tempdir().unwrap();

    let plugin = LoadedPluginV2 {
        id: crate::schemas::PluginId {
            name: "obj-plugin".to_string(),
            marketplace: "inline".to_string(),
        },
        manifest: crate::schemas::PluginManifestV2 {
            name: "obj-plugin".to_string(),
            commands: Some(ManifestCommands::ObjectMapping(HashMap::from([(
                "greet".to_string(),
                CommandMetadata {
                    source: None,
                    content: Some("Say hello to the user.".to_string()),
                    description: Some("Greeting command".to_string()),
                    argument_hint: None,
                    argument_kind: None,
                    model: None,
                    allowed_tools: None,
                },
            )]))),
            ..default_manifest_v2()
        },
        path: dir.path().to_path_buf(),
        load_source: crate::loader::PluginLoadSource::SessionDir,
        enabled: true,
    };

    let commands = load_plugin_commands_v2(&plugin);
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].base.name, "obj-plugin:greet");
    assert_eq!(commands[0].base.description, "Greeting command");
    assert_eq!(commands[0].prompt, "Say hello to the user.");
}

#[test]
fn test_load_v2_commands_from_manifest_object_source_path() {
    let dir = tempfile::tempdir().unwrap();

    std::fs::write(
        dir.path().join("review.md"),
        "---\ndescription: Review code\n---\nReview changes.\n",
    )
    .unwrap();

    let plugin = LoadedPluginV2 {
        id: crate::schemas::PluginId {
            name: "src-plugin".to_string(),
            marketplace: "inline".to_string(),
        },
        manifest: crate::schemas::PluginManifestV2 {
            name: "src-plugin".to_string(),
            commands: Some(ManifestCommands::ObjectMapping(HashMap::from([(
                "review".to_string(),
                CommandMetadata {
                    source: Some("review.md".to_string()),
                    content: None,
                    description: Some("Overridden description".to_string()),
                    argument_hint: None,
                    argument_kind: None,
                    model: None,
                    allowed_tools: None,
                },
            )]))),
            ..default_manifest_v2()
        },
        path: dir.path().to_path_buf(),
        load_source: crate::loader::PluginLoadSource::SessionDir,
        enabled: true,
    };

    let commands = load_plugin_commands_v2(&plugin);
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].base.name, "src-plugin:review");
    assert_eq!(commands[0].base.description, "Overridden description");
    assert_eq!(commands[0].prompt, "Review changes.");
}

#[test]
fn test_command_metadata_deserialize() {
    let json = serde_json::json!({
        "source": "cmd.md",
        "description": "A command",
        "argument_hint": "[file]",
        "argument_kind": "file_path",
        "model": "opus",
        "allowed_tools": ["Read", "Write"]
    });

    let meta: CommandMetadata = serde_json::from_value(json).unwrap();
    assert_eq!(meta.source.as_deref(), Some("cmd.md"));
    assert_eq!(meta.description.as_deref(), Some("A command"));
    assert_eq!(meta.argument_hint.as_deref(), Some("[file]"));
    assert_eq!(
        meta.argument_kind,
        Some(coco_types::CommandArgumentKind::FilePath)
    );
    assert_eq!(meta.model.as_deref(), Some("opus"));
    assert_eq!(
        meta.allowed_tools.as_deref(),
        Some(&["Read".to_string(), "Write".to_string()][..])
    );
}

#[test]
fn test_command_metadata_overrides() {
    let dir = tempfile::tempdir().unwrap();

    std::fs::write(
        dir.path().join("tool.md"),
        "---\ndescription: Original desc\nargument-hint: [arg]\nmodel: sonnet\n---\nTool prompt.\n",
    )
    .unwrap();

    let plugin = LoadedPluginV2 {
        id: crate::schemas::PluginId {
            name: "override-plugin".to_string(),
            marketplace: "inline".to_string(),
        },
        manifest: crate::schemas::PluginManifestV2 {
            name: "override-plugin".to_string(),
            commands: Some(ManifestCommands::ObjectMapping(HashMap::from([(
                "tool".to_string(),
                CommandMetadata {
                    source: Some("tool.md".to_string()),
                    content: None,
                    description: Some("Override desc".to_string()),
                    argument_hint: Some("[file]".to_string()),
                    argument_kind: Some(coco_types::CommandArgumentKind::FilePath),
                    model: Some("opus".to_string()),
                    allowed_tools: Some(vec!["Read".to_string()]),
                },
            )]))),
            ..default_manifest_v2()
        },
        path: dir.path().to_path_buf(),
        load_source: crate::loader::PluginLoadSource::SessionDir,
        enabled: true,
    };

    let commands = load_plugin_commands_v2(&plugin);
    assert_eq!(commands.len(), 1);
    let cmd = &commands[0];
    assert_eq!(cmd.base.description, "Override desc");
    assert_eq!(cmd.base.argument_hint.as_deref(), Some("[file]"));
    assert_eq!(
        cmd.base.argument_kind,
        coco_types::CommandArgumentKind::FilePath
    );

    if let coco_types::CommandType::Prompt(ref data) = cmd.command_type {
        assert_eq!(data.model.as_deref(), Some("opus"));
        assert_eq!(
            data.allowed_tools.as_deref(),
            Some(&["Read".to_string()][..])
        );
    } else {
        panic!("expected Prompt command type");
    }
}

#[test]
fn test_nonexistent_commands_dir_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    // No commands/ dir exists
    let plugin = test_plugin("no-cmds", dir.path());
    let commands = load_plugin_commands_v2(&plugin);
    assert!(commands.is_empty());
}

/// Helper to build a minimal V2 manifest for tests.
fn default_manifest_v2() -> crate::schemas::PluginManifestV2 {
    crate::schemas::PluginManifestV2 {
        name: String::new(),
        version: None,
        description: None,
        author: None,
        homepage: None,
        repository: None,
        license: None,
        keywords: None,
        dependencies: None,
        skills: None,
        hooks: None,
        agents: None,
        commands: None,
        mcp_servers: None,
        lsp_servers: None,
        output_styles: None,
        channels: None,
        user_config: None,
        settings: None,
        env_vars: None,
        min_version: None,
        max_version: None,
    }
}
