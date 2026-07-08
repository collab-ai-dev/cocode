use coco_config::EnvKey;
use coco_config::EnvSnapshot;
use coco_config::EventHubConfig;
use coco_config::RuntimeOverrides;
use coco_config::Settings;

#[test]
fn resolves_disabled_by_default() {
    let config = EventHubConfig::resolve(
        &Settings::default(),
        &EnvSnapshot::default(),
        &RuntimeOverrides::default(),
    )
    .unwrap();
    assert_eq!(config.url, None);
}

#[test]
fn settings_parser_accepts_event_hub_url() {
    let settings =
        coco_config::parse_settings(r#"{ "event_hub_url": "ws://hub/v1/connect" }"#).unwrap();

    assert_eq!(
        settings.event_hub_url.as_deref(),
        Some("ws://hub/v1/connect")
    );
}

#[test]
fn resolves_settings_env_and_cli_precedence() {
    let settings = Settings {
        event_hub_url: Some("ws://settings/v1/connect".to_string()),
        ..Default::default()
    };
    let env = EnvSnapshot::from_pairs([(EnvKey::CocoEventHubUrl, "ws://env/v1/connect")]);
    let overrides = RuntimeOverrides {
        event_hub_url_override: Some("wss://cli/v1/connect".to_string()),
        ..Default::default()
    };

    let config = EventHubConfig::resolve(&settings, &env, &overrides).unwrap();

    assert_eq!(config.url.as_deref(), Some("wss://cli/v1/connect"));
}

#[test]
fn rejects_non_websocket_url() {
    let settings = Settings {
        event_hub_url: Some("https://example.com/v1/connect".to_string()),
        ..Default::default()
    };

    let err = EventHubConfig::resolve(
        &settings,
        &EnvSnapshot::default(),
        &RuntimeOverrides::default(),
    )
    .unwrap_err();

    assert!(err.to_string().contains("event_hub_url must start"));
}
