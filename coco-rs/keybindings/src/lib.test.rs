use super::Keybinding;
use super::KeybindingAction;
use super::KeybindingBlock;
use super::KeybindingContext;
use super::KeybindingsConfig;
use std::collections::BTreeMap;

fn block_with(context: KeybindingContext, entries: &[(&str, KeybindingAction)]) -> KeybindingBlock {
    let mut bindings = BTreeMap::new();
    for (chord, action) in entries {
        bindings.insert((*chord).to_string(), Some(action.clone()));
    }
    KeybindingBlock { context, bindings }
}

#[test]
fn keybinding_constructors_parse_chord() {
    let kb = Keybinding::new(
        "ctrl+c",
        KeybindingAction::AppInterrupt,
        KeybindingContext::Global,
    )
    .unwrap();
    assert_eq!(kb.action, Some(KeybindingAction::AppInterrupt));
    assert_eq!(kb.context, KeybindingContext::Global);
    assert!(kb.chord.is_single());
}

#[test]
fn keybinding_unbind_has_none_action() {
    let kb = Keybinding::unbind("ctrl+c", KeybindingContext::Chat).unwrap();
    assert_eq!(kb.action, None);
}

#[test]
fn config_from_json_parses_object_wrapper() {
    let json = r#"{
        "$schema": "https://example/schema.json",
        "$docs": "https://example/docs",
        "bindings": [
            {
                "context": "Chat",
                "bindings": {
                    "ctrl+c": "chat:cancel",
                    "ctrl+x ctrl+k": null
                }
            }
        ]
    }"#;

    let config = KeybindingsConfig::from_json(json).unwrap();
    assert_eq!(
        config.schema.as_deref(),
        Some("https://example/schema.json")
    );
    assert_eq!(config.docs.as_deref(), Some("https://example/docs"));
    assert_eq!(config.bindings.len(), 1);

    let block = &config.bindings[0];
    assert_eq!(block.context, KeybindingContext::Chat);
    assert_eq!(
        block.bindings.get("ctrl+c"),
        Some(&Some(KeybindingAction::ChatCancel)),
    );
    assert_eq!(block.bindings.get("ctrl+x ctrl+k"), Some(&None));
}

#[test]
fn config_to_json_pretty_round_trip() {
    let config = KeybindingsConfig {
        schema: None,
        docs: None,
        bindings: vec![block_with(
            KeybindingContext::Global,
            &[("ctrl+c", KeybindingAction::AppInterrupt)],
        )],
    };

    let json = config.to_json_pretty().unwrap();
    assert!(json.ends_with('\n'), "trailing newline expected");
    let reparsed = KeybindingsConfig::from_json(&json).unwrap();
    assert_eq!(reparsed, config);
}

#[test]
fn parse_bindings_skips_unparseable_chord() {
    let mut bindings = BTreeMap::new();
    bindings.insert("ctrl+c".to_string(), Some(KeybindingAction::AppInterrupt));
    bindings.insert(String::new(), Some(KeybindingAction::AppExit));
    let config = KeybindingsConfig {
        schema: None,
        docs: None,
        bindings: vec![KeybindingBlock {
            context: KeybindingContext::Global,
            bindings,
        }],
    };

    let parsed = config.parse_bindings();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].action, Some(KeybindingAction::AppInterrupt));
}

#[test]
fn parse_bindings_preserves_unbind() {
    let mut bindings = BTreeMap::new();
    bindings.insert("ctrl+c".to_string(), None);
    let config = KeybindingsConfig {
        schema: None,
        docs: None,
        bindings: vec![KeybindingBlock {
            context: KeybindingContext::Chat,
            bindings,
        }],
    };

    let parsed = config.parse_bindings();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].action, None);
}

#[test]
fn config_default_is_empty() {
    let config = KeybindingsConfig::default();
    assert!(config.schema.is_none());
    assert!(config.docs.is_none());
    assert!(config.bindings.is_empty());
}
