use std::collections::HashMap;
#[cfg(unix)]
use std::path::PathBuf;

use coco_cli::headless::build_system_prompt_for_model;
use coco_config::CatalogPaths;
use coco_config::EnvSnapshot;
use coco_config::RoleSlots;
use coco_config::RuntimeConfig;
use coco_config::RuntimeOverrides;
use coco_config::Settings;
use coco_config::SettingsWithSource;
use coco_types::ProviderModelSelection;
use tempfile::TempDir;

fn runtime_for_model(selection: &str, home: &TempDir) -> RuntimeConfig {
    let settings = SettingsWithSource {
        merged: Settings {
            models: coco_config::ModelSelectionSettings {
                main: Some(RoleSlots::new(
                    ProviderModelSelection::from_slash_str(selection).expect("model selection"),
                )),
                ..Default::default()
            },
            ..Default::default()
        },
        per_source: HashMap::new(),
        source_paths: HashMap::new(),
    };
    coco_config::build_runtime_config_with(
        settings,
        EnvSnapshot::default(),
        RuntimeOverrides::default(),
        CatalogPaths::empty_in(home.path()),
        coco_config::parse_enabled_setting_sources(None),
    )
    .expect("runtime config")
}

#[test]
fn build_system_prompt_uses_model_instructions_when_present() {
    let home = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let runtime = runtime_for_model("openai/gpt-5-4", &home);

    let prompt =
        build_system_prompt_for_model(cwd.path(), &runtime, "openai", "gpt-5-4", None, &[]);

    assert!(
        prompt.starts_with(&format!(
            "You are {}, a coding agent based on GPT-5.",
            coco_config::constants::PRODUCT_NAME
        )),
        "shared headless/SDK/TUI prompt builder should use model instructions"
    );
    assert!(prompt.contains("# Personality"));
    assert!(!prompt.starts_with(&coco_config::default_base_instructions()));
}

#[test]
fn build_system_prompt_falls_back_when_model_has_no_instructions() {
    let home = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let runtime = runtime_for_model("anthropic/claude-sonnet-4-6", &home);

    let prompt = build_system_prompt_for_model(
        cwd.path(),
        &runtime,
        "anthropic",
        "claude-sonnet-4-6",
        None,
        &[],
    );

    assert!(prompt.starts_with(&coco_config::default_base_instructions()));
    assert!(prompt.contains("<env>"));
}

#[cfg(unix)]
#[test]
fn sdk_unix_socket_path_reads_runtime_server_config() {
    let home = TempDir::new().unwrap();
    let mut runtime = runtime_for_model("openai/gpt-5-4", &home);
    runtime.server.unix_socket_path = Some(" /tmp/coco-sdk.sock ".to_string());

    assert_eq!(
        super::sdk_unix_socket_path(&runtime),
        Some(PathBuf::from("/tmp/coco-sdk.sock"))
    );
}

#[cfg(unix)]
#[test]
fn sdk_unix_socket_path_ignores_empty_runtime_server_config() {
    let home = TempDir::new().unwrap();
    let mut runtime = runtime_for_model("openai/gpt-5-4", &home);
    runtime.server.unix_socket_path = Some("   ".to_string());

    assert_eq!(super::sdk_unix_socket_path(&runtime), None);
}

#[test]
fn sdk_websocket_bind_reads_runtime_server_config() {
    let home = TempDir::new().unwrap();
    let mut runtime = runtime_for_model("openai/gpt-5-4", &home);
    runtime.server.websocket_bind = Some(" 127.0.0.1:7777 ".to_string());

    assert_eq!(
        super::sdk_websocket_bind(&runtime).as_deref(),
        Some("127.0.0.1:7777")
    );
}

#[test]
fn sdk_websocket_bind_ignores_empty_runtime_server_config() {
    let home = TempDir::new().unwrap();
    let mut runtime = runtime_for_model("openai/gpt-5-4", &home);
    runtime.server.websocket_bind = Some("   ".to_string());

    assert_eq!(super::sdk_websocket_bind(&runtime), None);
}

#[test]
fn sdk_named_pipe_name_reads_runtime_server_config() {
    let home = TempDir::new().unwrap();
    let mut runtime = runtime_for_model("openai/gpt-5-4", &home);
    runtime.server.named_pipe_name = Some(r"  \\.\pipe\coco-sdk  ".to_string());

    assert_eq!(
        super::sdk_named_pipe_name(&runtime).as_deref(),
        Some(r"\\.\pipe\coco-sdk")
    );
}

#[test]
fn sdk_named_pipe_name_ignores_empty_runtime_server_config() {
    let home = TempDir::new().unwrap();
    let mut runtime = runtime_for_model("openai/gpt-5-4", &home);
    runtime.server.named_pipe_name = Some("   ".to_string());

    assert_eq!(super::sdk_named_pipe_name(&runtime), None);
}
