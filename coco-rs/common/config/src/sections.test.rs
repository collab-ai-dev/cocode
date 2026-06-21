use pretty_assertions::assert_eq;

use super::*;
use crate::EnvKey;
use crate::EnvSnapshot;
use crate::settings::Settings;

#[test]
fn test_agent_teams_config_defaults_to_main_model_role() {
    let missing = AgentTeamsConfig::resolve(&Settings::default()).unwrap();
    assert_eq!(missing.default_model_role, coco_types::ModelRole::Main);
    assert!(missing.agent_type_model_roles.is_empty());
    assert_eq!(missing.default_model, None);
}

#[test]
fn test_agent_teams_config_resolves_role_overrides() {
    let config = AgentTeamsConfig::resolve(&Settings {
        agent_teams: PartialAgentTeamsSettings {
            default_model_role: Some(coco_types::ModelRole::Fast),
            agent_type_model_roles: Some(
                [("reviewer".to_string(), coco_types::ModelRole::Review)]
                    .into_iter()
                    .collect(),
            ),
            ..Default::default()
        },
        ..Default::default()
    })
    .unwrap();
    assert_eq!(config.default_model_role, coco_types::ModelRole::Fast);
    assert_eq!(
        config.agent_type_model_roles.get("reviewer"),
        Some(&coco_types::ModelRole::Review)
    );
}

#[test]
fn test_agent_teams_config_resolves_concrete_default_model() {
    let config = AgentTeamsConfig::resolve(&Settings {
        agent_teams: PartialAgentTeamsSettings {
            default_model: Some(coco_types::ProviderModelSelection {
                provider: "openai".into(),
                model_id: "gpt-5-5".into(),
            }),
            ..Default::default()
        },
        ..Default::default()
    })
    .unwrap();
    assert_eq!(
        config.default_model,
        Some(coco_types::ProviderModelSelection {
            provider: "openai".into(),
            model_id: "gpt-5-5".into(),
        })
    );
}

#[test]
fn test_agent_teams_config_rejects_removed_teammate_role() {
    let err = serde_json::from_value::<Settings>(serde_json::json!({
        "agent_teams": {
            "default_model_role": "teammate"
        }
    }))
    .expect_err("teammate role must not parse");
    assert!(
        err.to_string().contains("unknown variant")
            || err.to_string().contains("unknown model role"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_bash_config_finalize_clamps_max_output_bytes() {
    let settings = Settings {
        tool: PartialToolSettings {
            bash: Some(PartialBashSettings {
                max_output_bytes: Some(999_999),
                ..Default::default()
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    let config = ToolConfig::resolve(&settings, &EnvSnapshot::default());
    assert_eq!(
        config.bash.max_output_bytes,
        crate::sections::BASH_MAX_OUTPUT_BYTES_UPPER
    );
}

#[test]
fn test_bash_config_finalize_rejects_negative_max_output_bytes() {
    let settings = Settings {
        tool: PartialToolSettings {
            bash: Some(PartialBashSettings {
                max_output_bytes: Some(-5),
                ..Default::default()
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    let config = ToolConfig::resolve(&settings, &EnvSnapshot::default());
    assert_eq!(config.bash.max_output_bytes, 0);
}

#[test]
fn test_tool_config_json_first_env_override() {
    let settings = Settings {
        tool: PartialToolSettings {
            max_tool_concurrency: Some(4),
            glob_timeout_seconds: Some(12),
            bash: Some(PartialBashSettings {
                auto_background_on_timeout: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    let env = EnvSnapshot::from_pairs([
        (EnvKey::CocoMaxToolUseConcurrency, "8"),
        (EnvKey::CocoBashAutoBackgroundOnTimeout, "1"),
    ]);

    let config = ToolConfig::resolve(&settings, &env);

    assert_eq!(config.max_tool_concurrency, 8);
    assert_eq!(config.glob_timeout_seconds, 12);
    assert!(config.bash.auto_background_on_timeout);
}

#[test]
fn shell_config_defaults_tool_to_auto() {
    let config = ShellConfig::resolve(&Settings::default(), &EnvSnapshot::default());
    assert_eq!(config.tool, ShellToolSelection::Auto);
}

#[test]
fn shell_config_parses_tool_selection() {
    let settings = Settings {
        shell: PartialShellSettings {
            tool: Some(ShellToolSelection::PowerShell),
            ..Default::default()
        },
        ..Default::default()
    };
    let config = ShellConfig::resolve(&settings, &EnvSnapshot::default());
    assert_eq!(config.tool, ShellToolSelection::PowerShell);
}

#[test]
fn test_api_retry_env_max_retries_overrides_settings() {
    let settings = Settings {
        api: PartialApiSettings {
            retry: Some(PartialApiRetrySettings {
                max_retries: Some(3),
                base_delay_ms: Some(750),
                max_delay_ms: Some(500),
                ..Default::default()
            }),
        },
        ..Default::default()
    };
    let env = EnvSnapshot::from_pairs([(EnvKey::CocoApiMaxRetries, "12")]);

    let config = ApiConfig::resolve(&settings, &env);

    assert_eq!(config.retry.max_retries, 12);
    assert_eq!(config.retry.base_delay_ms, 750);
    assert_eq!(config.retry.max_delay_ms, 750);
}

#[test]
fn test_api_retry_env_max_retries_is_clamped() {
    let env = EnvSnapshot::from_pairs([(EnvKey::CocoApiMaxRetries, "-4")]);

    let config = ApiConfig::resolve(&Settings::default(), &env);

    assert_eq!(config.retry.max_retries, 0);
}

#[test]
fn test_memory_config_resolves_sub_toggles() {
    // After feature-gate consolidation, top-level enable/disable lives on
    // `Feature::AutoMemory`, not on `MemoryConfig`. This struct only carries
    // sub-toggles + parameters.
    let settings = Settings {
        memory: PartialMemorySettings {
            extraction_enabled: Some(false),
            team_memory_enabled: Some(true),
            ..Default::default()
        },
        ..Default::default()
    };
    let env = EnvSnapshot::from_pairs(std::iter::empty::<(EnvKey, &str)>());

    let config = MemoryConfig::resolve(&settings, &env);

    assert!(!config.extraction_enabled);
    assert!(config.team_memory_enabled);
}

#[test]
fn test_sandbox_settings_resolves_mode_and_network() {
    // After feature-gate consolidation, top-level enable/disable lives on
    // `Feature::Sandbox`. The mode + network toggles are coco-rs-specific
    // posture knobs layered on top of the rich `SandboxSettings`.
    let settings = Settings {
        sandbox: crate::sandbox_settings::SandboxSettings {
            mode: coco_types::SandboxMode::ReadOnly,
            allow_network: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let env = EnvSnapshot::from_pairs([(EnvKey::CocoSandboxMode, "workspace-write")]);

    let config = crate::sandbox_settings::SandboxSettings::resolve(&settings, &env);

    // Env override beats settings.
    assert_eq!(config.mode, coco_types::SandboxMode::WorkspaceWrite);
    assert!(config.allow_network);
}

#[test]
fn test_mcp_runtime_config_json_first_env_override() {
    let settings = Settings {
        mcp_runtime: PartialMcpRuntimeSettings {
            tool_timeout_ms: Some(5_000),
        },
        ..Default::default()
    };
    let env = EnvSnapshot::from_pairs([(EnvKey::CocoMcpToolTimeoutMs, "2500")]);

    let config = McpRuntimeConfig::resolve(&settings, &env);

    assert_eq!(config.tool_timeout_ms, Some(2_500));
}
