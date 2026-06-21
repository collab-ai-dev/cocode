use super::*;
use coco_subagent::definition_store::AgentSearchPaths;
use std::sync::Arc;

#[tokio::test]
async fn empty_registry_returns_empty_commands() {
    let boot = CliInitializeBootstrap::new("default".into());
    assert!(boot.commands().await.is_empty());
}

#[tokio::test]
async fn commands_walks_registry_visible_list() {
    use coco_commands::{BuiltinCommand, RegisteredCommand};
    use coco_types::{CommandArgumentKind, CommandBase, CommandSafety, CommandType};

    let mut registry = CommandRegistry::new();
    registry.register(RegisteredCommand {
        base: CommandBase {
            name: "visible-cmd".into(),
            description: "A visible command".into(),
            aliases: Vec::new(),
            availability: Vec::new(),
            is_hidden: false,
            argument_hint: Some("<arg>".into()),
            argument_kind: CommandArgumentKind::FreeText,
            when_to_use: None,
            user_invocable: true,
            is_sensitive: false,
            loaded_from: None,
            safety: CommandSafety::AlwaysSafe,
            supports_non_interactive: false,
        },
        command_type: CommandType::Local(coco_types::LocalCommandData {
            handler: "test".into(),
        }),
        handler: Some(Arc::new(BuiltinCommand::new("visible-cmd", |_| {
            String::new()
        }))),
        is_enabled: None,
    });
    registry.register(RegisteredCommand {
        base: CommandBase {
            name: "hidden-cmd".into(),
            description: "A hidden command".into(),
            aliases: Vec::new(),
            availability: Vec::new(),
            is_hidden: true,
            argument_hint: None,
            argument_kind: CommandArgumentKind::None,
            when_to_use: None,
            user_invocable: false,
            is_sensitive: false,
            loaded_from: None,
            safety: CommandSafety::AlwaysSafe,
            supports_non_interactive: false,
        },
        command_type: CommandType::Local(coco_types::LocalCommandData {
            handler: "test".into(),
        }),
        handler: Some(Arc::new(BuiltinCommand::new("hidden-cmd", |_| {
            String::new()
        }))),
        is_enabled: None,
    });

    let boot = CliInitializeBootstrap::new("default".into())
        .with_command_registry(Arc::new(tokio::sync::RwLock::new(Arc::new(registry))));
    let commands = boot.commands().await;
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].name, "visible-cmd");
    assert_eq!(commands[0].description, "A visible command");
    assert_eq!(commands[0].argument_hint, "<arg>");
}

#[tokio::test]
async fn commands_filters_sensitive_commands() {
    use coco_commands::{BuiltinCommand, RegisteredCommand};
    use coco_types::{CommandArgumentKind, CommandBase, CommandSafety, CommandType};

    let mut registry = CommandRegistry::new();
    // Non-sensitive: must appear.
    registry.register(RegisteredCommand {
        base: CommandBase {
            name: "public-cmd".into(),
            description: "Public command".into(),
            aliases: Vec::new(),
            availability: Vec::new(),
            is_hidden: false,
            argument_hint: None,
            argument_kind: CommandArgumentKind::None,
            when_to_use: None,
            user_invocable: true,
            is_sensitive: false,
            loaded_from: None,
            safety: CommandSafety::AlwaysSafe,
            supports_non_interactive: false,
        },
        command_type: CommandType::Local(coco_types::LocalCommandData {
            handler: "test".into(),
        }),
        handler: Some(Arc::new(BuiltinCommand::new("public-cmd", |_| {
            String::new()
        }))),
        is_enabled: None,
    });
    // Sensitive: must NOT appear on the SDK wire even though
    // `is_hidden: false` would pass the looser `visible()` filter.
    registry.register(RegisteredCommand {
        base: CommandBase {
            name: "secret-cmd".into(),
            description: "Sensitive internal command".into(),
            aliases: Vec::new(),
            availability: Vec::new(),
            is_hidden: false,
            argument_hint: None,
            argument_kind: CommandArgumentKind::None,
            when_to_use: None,
            user_invocable: true,
            is_sensitive: true,
            loaded_from: None,
            safety: CommandSafety::AlwaysSafe,
            supports_non_interactive: false,
        },
        command_type: CommandType::Local(coco_types::LocalCommandData {
            handler: "test".into(),
        }),
        handler: Some(Arc::new(BuiltinCommand::new("secret-cmd", |_| {
            String::new()
        }))),
        is_enabled: None,
    });

    let boot = CliInitializeBootstrap::new("default".into())
        .with_command_registry(Arc::new(tokio::sync::RwLock::new(Arc::new(registry))));
    let commands = boot.commands().await;
    let names: Vec<&str> = commands.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"public-cmd"));
    assert!(
        !names.contains(&"secret-cmd"),
        "sensitive commands must not be advertised to SDK clients"
    );
}

#[tokio::test]
async fn missing_argument_hint_becomes_empty_string() {
    use coco_commands::{BuiltinCommand, RegisteredCommand};
    use coco_types::{CommandArgumentKind, CommandBase, CommandSafety, CommandType};

    let mut registry = CommandRegistry::new();
    registry.register(RegisteredCommand {
        base: CommandBase {
            name: "no-hint".into(),
            description: "Command without argument hint".into(),
            aliases: Vec::new(),
            availability: Vec::new(),
            is_hidden: false,
            argument_hint: None,
            argument_kind: CommandArgumentKind::None,
            when_to_use: None,
            user_invocable: true,
            is_sensitive: false,
            loaded_from: None,
            safety: CommandSafety::AlwaysSafe,
            supports_non_interactive: false,
        },
        command_type: CommandType::Local(coco_types::LocalCommandData {
            handler: "test".into(),
        }),
        handler: Some(Arc::new(BuiltinCommand::new("no-hint", |_| String::new()))),
        is_enabled: None,
    });

    let boot = CliInitializeBootstrap::new("default".into())
        .with_command_registry(Arc::new(tokio::sync::RwLock::new(Arc::new(registry))));
    let commands = boot.commands().await;
    assert_eq!(commands.len(), 1);
    // `argumentHint` is REQUIRED on the wire; we default to empty string when None.
    assert_eq!(commands[0].argument_hint, "");
}

#[tokio::test]
async fn output_style_round_trip() {
    // Built-in style names are `default`, `Explanatory`,
    // `Learning` — verify round-trip preserves case.
    let boot = CliInitializeBootstrap::new("Explanatory".into());
    assert_eq!(boot.output_style().await, "Explanatory");
}

#[tokio::test]
async fn available_output_styles_defaults_to_builtins() {
    let boot = CliInitializeBootstrap::new("default".into());
    let styles = boot.available_output_styles().await;
    for builtin in BUILTIN_OUTPUT_STYLES {
        assert!(
            styles.contains(&builtin.to_string()),
            "missing builtin {builtin}"
        );
    }
}

#[tokio::test]
async fn with_available_output_styles_replaces_default_list() {
    // Caller-provided list (CLI passes the resolved `OutputStyleManager`
    // catalog + the `default` sentinel) is returned verbatim. Aggregation
    // and dir / plugin walking lives in `coco-output-styles`, not here.
    let boot =
        CliInitializeBootstrap::new("custom-style".into()).with_available_output_styles(vec![
            "default".into(),
            "Explanatory".into(),
            "Learning".into(),
            "custom-style".into(),
            "alpha:plugin-style".into(),
        ]);
    let styles = boot.available_output_styles().await;
    assert_eq!(
        styles,
        vec![
            "default".to_string(),
            "Explanatory".into(),
            "Learning".into(),
            "custom-style".into(),
            "alpha:plugin-style".into(),
        ]
    );
}

#[tokio::test]
async fn account_defaults_without_auth_method() {
    let boot = CliInitializeBootstrap::new("default".into());
    let account = boot.account().await;
    assert!(account.email.is_none());
    assert!(account.api_provider.is_none());
}

#[tokio::test]
async fn account_api_key_maps_to_first_party() {
    use coco_inference::auth::AuthMethod;
    let boot = CliInitializeBootstrap::new("default".into()).with_auth_method(AuthMethod::ApiKey {
        key: "sk-ant-...".into(),
    });
    let account = boot.account().await;
    assert_eq!(account.api_provider, Some(SdkApiProvider::FirstParty));
    // `token_source` is intentionally None — we leave the field absent
    // rather than send a string clients won't recognize.
    assert!(account.token_source.is_none());
    assert!(account.email.is_none());
    // Key is NEVER exposed on the wire.
}

#[tokio::test]
async fn account_oauth_maps_subscription_and_org() {
    use coco_inference::auth::{AuthMethod, OAuthTokens};
    let boot = CliInitializeBootstrap::new("default".into()).with_auth_method(AuthMethod::OAuth(
        OAuthTokens {
            access_token: "tok".into(),
            refresh_token: None,
            expires_at: None,
            subscription_type: Some("max".into()),
            org_uuid: Some("org-uuid-123".into()),
        },
    ));
    let account = boot.account().await;
    assert_eq!(account.api_provider, Some(SdkApiProvider::FirstParty));
    assert_eq!(account.subscription_type.as_deref(), Some("max"));
    assert_eq!(account.organization.as_deref(), Some("org-uuid-123"));
    // See note above — token_source is intentionally None.
    assert!(account.token_source.is_none());
}

#[tokio::test]
async fn account_oauth_does_not_leak_access_token() {
    use coco_inference::auth::{AuthMethod, OAuthTokens};
    let boot = CliInitializeBootstrap::new("default".into()).with_auth_method(AuthMethod::OAuth(
        OAuthTokens {
            access_token: "SECRET-ACCESS-TOKEN-DO-NOT-LEAK".into(),
            refresh_token: Some("SECRET-REFRESH-TOKEN".into()),
            expires_at: Some(1_700_000_000),
            subscription_type: Some("max".into()),
            org_uuid: Some("org-uuid-123".into()),
        },
    ));
    let account = boot.account().await;
    // Serialize and search for secret material — regression guard
    // against accidentally adding a field that forwards the token.
    let json = serde_json::to_string(&account).unwrap();
    assert!(!json.contains("SECRET-ACCESS-TOKEN"));
    assert!(!json.contains("SECRET-REFRESH-TOKEN"));
    assert!(!json.contains("1700000000"));
}

#[tokio::test]
async fn account_third_party_providers_return_empty_account() {
    // Non-FirstParty providers return a bare `SdkAccountInfo::default()`
    // which serializes to `{}` — the closest analogue for an optional
    // `account` field on the wire.
    use coco_inference::auth::AuthMethod;

    for auth in [
        AuthMethod::Bedrock {
            region: "us-east-1".into(),
            profile: None,
        },
        AuthMethod::Vertex {
            project_id: "proj".into(),
            region: "us-central1".into(),
        },
        AuthMethod::Foundry {
            endpoint: "https://example".into(),
        },
    ] {
        let account = CliInitializeBootstrap::new("default".into())
            .with_auth_method(auth)
            .account()
            .await;
        assert!(account.api_provider.is_none());
        assert!(account.organization.is_none());
        assert!(account.email.is_none());
        assert!(account.subscription_type.is_none());
        assert!(account.token_source.is_none());
        assert!(account.api_key_source.is_none());
    }
}

#[tokio::test]
async fn agents_returns_builtins_when_no_dirs_configured() {
    let boot = CliInitializeBootstrap::new("default".into());
    let agents = boot.agents().await;
    // `BuiltinAgentCatalog::interactive()` enables general-purpose +
    // statusline-setup + Explore + Plan + coco-guide (5 entries).
    // Verification ships behind a separate gate (`include_verification`)
    // and is not part of the interactive default. Assert presence of the
    // canonical names so adding verification later (or any future
    // built-in) only relaxes the count without invalidating the test.
    let names: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
    assert!(
        agents.len() >= 5,
        "expected at least 5 built-in agents in interactive mode, got {}: {names:?}",
        agents.len()
    );
    assert!(names.contains(&"general-purpose"));
    assert!(names.contains(&"Explore"));
    assert!(names.contains(&"Plan"));
    assert!(names.contains(&"statusline-setup"));
    assert!(names.contains(&"coco-guide"));
    // Descriptions come through non-empty for every built-in.
    for agent in &agents {
        assert!(
            !agent.description.is_empty(),
            "agent {} has empty description",
            agent.name
        );
    }
}

#[tokio::test]
async fn agents_user_definition_overrides_builtin_by_name() {
    // Built-in `Explore` has a fixed description. A user-defined
    // markdown with `name: Explore` should replace the built-in's
    // description in the merged list.
    let tempdir = tempfile::tempdir().unwrap();
    let override_md = r#"---
name: Explore
description: Override description from user file
---
Body not used for this assertion.
"#;
    std::fs::write(tempdir.path().join("Explore.md"), override_md).unwrap();

    let boot =
        CliInitializeBootstrap::new("default".into()).with_agent_search_paths(AgentSearchPaths {
            user_dir: Some(tempdir.path().to_path_buf()),
            ..AgentSearchPaths::empty()
        });
    let agents = boot.agents().await;
    let explore = agents
        .iter()
        .find(|a| a.name == "Explore")
        .expect("Explore should still be present");
    assert_eq!(explore.description, "Override description from user file");
    // There's still only ONE `Explore` — no duplication.
    let explore_count = agents.iter().filter(|a| a.name == "Explore").count();
    assert_eq!(explore_count, 1);
}

#[tokio::test]
async fn agents_merges_custom_directory_definitions() {
    let tempdir = tempfile::tempdir().unwrap();
    let custom_md = r#"---
name: custom-researcher
description: A user-defined researcher agent
model: claude-sonnet-4-6
---
You are a research agent. Find information thoroughly.
"#;
    std::fs::write(tempdir.path().join("custom-researcher.md"), custom_md).unwrap();

    let boot =
        CliInitializeBootstrap::new("default".into()).with_agent_search_paths(AgentSearchPaths {
            user_dir: Some(tempdir.path().to_path_buf()),
            ..AgentSearchPaths::empty()
        });
    let agents = boot.agents().await;
    let custom = agents
        .iter()
        .find(|a| a.name == "custom-researcher")
        .expect("custom-researcher should be present");
    assert_eq!(custom.description, "A user-defined researcher agent");
    assert_eq!(custom.model.as_deref(), Some("claude-sonnet-4-6"));
    // Built-ins still present alongside the custom one.
    assert!(agents.iter().any(|a| a.name == "general-purpose"));
}

#[tokio::test]
async fn fast_mode_state_stub_returns_none() {
    let boot = CliInitializeBootstrap::new("default".into());
    assert!(boot.fast_mode_state().await.is_none());
}
