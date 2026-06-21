use super::KeybindingAction;
use super::UnknownActionReason;

#[test]
fn parse_known_actions_round_trip() {
    // Sample one variant from each namespace to ensure both halves of
    // `as_str` and `FromStr` agree across the table.
    let names = [
        "app:exit",
        "history:search",
        "chat:submit",
        "autocomplete:accept",
        "confirm:yes",
        "tabs:next",
        "transcript:exit",
        "historySearch:next",
        "task:background",
        "theme:toggleSyntaxHighlighting",
        "help:dismiss",
        "attachments:next",
        "footer:up",
        "messageSelector:select",
        "diff:dismiss",
        "modelPicker:decreaseEffort",
        "select:accept",
        "plugin:toggle",
        "permission:toggleDebug",
        "settings:close",
        "voice:pushToTalk",
        "scroll:pageUp",
        "selection:copy",
        "messageActions:enter",
        // coco-rs-only team-roster open action (A7a) — must round-trip so it
        // loads from `~/.coco/keybindings.json`.
        "app:toggleTeamRoster",
        // coco-rs extensions folded from the old TUI cascade (tui-v2) — each
        // must round-trip so a `~/.coco/keybindings.json` override loads.
        "app:forceQuit",
        "app:help",
        "app:commandPalette",
        "app:settings",
        "app:sessionBrowser",
        "app:planEditor",
        "chat:toggleSystemReminders",
        "chat:togglePlanMode",
    ];
    for name in names {
        let action: KeybindingAction = name.parse().expect(name);
        assert_eq!(action.to_string(), name, "round-trip {name}");
    }
}

#[test]
fn parse_command_escape_hatch() {
    let cases = [
        ("command:help", "help"),
        ("command:compact", "compact"),
        ("command:my-cmd_2", "my-cmd_2"),
        ("command:nested:thing", "nested:thing"),
    ];
    for (input, expected_inner) in cases {
        let action: KeybindingAction = input.parse().unwrap();
        assert!(action.is_command());
        assert_eq!(action.to_string(), input);
        match action {
            KeybindingAction::Command(inner) => assert_eq!(inner, expected_inner),
            _ => panic!("expected Command variant"),
        }
    }
}

#[test]
fn rejects_unknown_action() {
    let err = "app:no-such-action"
        .parse::<KeybindingAction>()
        .unwrap_err();
    assert_eq!(err.reason, UnknownActionReason::NotARecognizedAction);
}

#[test]
fn rejects_invalid_command_name() {
    // Empty after `command:` is invalid per the regex.
    assert_eq!(
        "command:".parse::<KeybindingAction>().unwrap_err().reason,
        UnknownActionReason::InvalidCommandName,
    );
    // Disallowed character — space.
    assert_eq!(
        "command:hello world"
            .parse::<KeybindingAction>()
            .unwrap_err()
            .reason,
        UnknownActionReason::InvalidCommandName,
    );
    // Disallowed character — slash.
    assert_eq!(
        "command:foo/bar"
            .parse::<KeybindingAction>()
            .unwrap_err()
            .reason,
        UnknownActionReason::InvalidCommandName,
    );
}

#[test]
fn serde_round_trip_via_string() {
    let action = KeybindingAction::ChatSubmit;
    let json = serde_json::to_string(&action).unwrap();
    assert_eq!(json, "\"chat:submit\"");

    let parsed: KeybindingAction = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, action);
}

#[test]
fn serde_round_trip_command_variant() {
    let action = KeybindingAction::Command("my-cmd".to_string());
    let json = serde_json::to_string(&action).unwrap();
    assert_eq!(json, "\"command:my-cmd\"");
    let parsed: KeybindingAction = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, action);
}

#[test]
fn serde_rejects_unknown() {
    let result: Result<KeybindingAction, _> = serde_json::from_str("\"app:no-such-action\"");
    assert!(result.is_err());
}
