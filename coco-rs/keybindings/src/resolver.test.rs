use super::ChordResolver;
use super::ResolveOutcome;
use crate::Keybinding;
use crate::KeybindingAction;
use crate::KeybindingContext;
use crate::parser::parse_combo;

fn binding(chord: &str, action: KeybindingAction, context: KeybindingContext) -> Keybinding {
    Keybinding::new(chord, action, context).expect("test chord parses")
}

fn unbind(chord: &str, context: KeybindingContext) -> Keybinding {
    Keybinding::unbind(chord, context).expect("test chord parses")
}

#[test]
fn single_combo_fires_immediately() {
    let bindings = vec![binding(
        "ctrl+a",
        KeybindingAction::SelectAccept,
        KeybindingContext::Chat,
    )];
    let mut resolver = ChordResolver::new(&bindings);
    let combo = parse_combo("ctrl+a").unwrap();
    assert_eq!(
        resolver.feed(&combo, &[KeybindingContext::Chat]),
        ResolveOutcome::Fire(KeybindingAction::SelectAccept),
    );
    assert!(!resolver.has_pending());
}

#[test]
fn unknown_combo_returns_no_match() {
    let bindings = vec![binding(
        "ctrl+a",
        KeybindingAction::SelectAccept,
        KeybindingContext::Chat,
    )];
    let mut resolver = ChordResolver::new(&bindings);
    let combo = parse_combo("ctrl+b").unwrap();
    assert_eq!(
        resolver.feed(&combo, &[KeybindingContext::Chat]),
        ResolveOutcome::NoMatch,
    );
}

#[test]
fn chord_returns_pending_then_fire() {
    let bindings = vec![binding(
        "ctrl+k ctrl+s",
        KeybindingAction::ChatStash,
        KeybindingContext::Chat,
    )];
    let mut resolver = ChordResolver::new(&bindings);
    let first = parse_combo("ctrl+k").unwrap();
    let second = parse_combo("ctrl+s").unwrap();
    assert_eq!(
        resolver.feed(&first, &[KeybindingContext::Chat]),
        ResolveOutcome::Pending,
    );
    assert!(resolver.has_pending());
    assert_eq!(
        resolver.feed(&second, &[KeybindingContext::Chat]),
        ResolveOutcome::Fire(KeybindingAction::ChatStash),
    );
    assert!(!resolver.has_pending());
}

// Replaced by `unmatched_followup_returns_chord_cancelled` after P3 —
// pending chord broken by an unmatched follow-up now yields
// `ChordCancelled` instead of `NoMatch`.

#[test]
fn context_specificity_wins() {
    let bindings = vec![
        binding(
            "enter",
            KeybindingAction::ConfirmYes,
            KeybindingContext::Global,
        ),
        binding(
            "enter",
            KeybindingAction::ChatSubmit,
            KeybindingContext::Chat,
        ),
    ];
    let mut resolver = ChordResolver::new(&bindings);
    let combo = parse_combo("enter").unwrap();

    // With Chat in front of Global, Chat wins.
    assert_eq!(
        resolver.feed(
            &combo,
            &[KeybindingContext::Chat, KeybindingContext::Global]
        ),
        ResolveOutcome::Fire(KeybindingAction::ChatSubmit),
    );

    // With only Global in the stack, Global wins.
    assert_eq!(
        resolver.feed(&combo, &[KeybindingContext::Global]),
        ResolveOutcome::Fire(KeybindingAction::ConfirmYes),
    );
}

#[test]
fn null_unbind_returns_unbound_outcome() {
    let bindings = vec![unbind("ctrl+t", KeybindingContext::Chat)];
    let mut resolver = ChordResolver::new(&bindings);
    let combo = parse_combo("ctrl+t").unwrap();
    assert_eq!(
        resolver.feed(&combo, &[KeybindingContext::Chat]),
        ResolveOutcome::Unbound,
    );
}

#[test]
fn reset_clears_pending_state() {
    let bindings = vec![binding(
        "ctrl+k ctrl+s",
        KeybindingAction::ChatStash,
        KeybindingContext::Chat,
    )];
    let mut resolver = ChordResolver::new(&bindings);
    let first = parse_combo("ctrl+k").unwrap();
    let second = parse_combo("ctrl+s").unwrap();
    resolver.feed(&first, &[KeybindingContext::Chat]);
    resolver.reset();
    assert_eq!(
        resolver.feed(&second, &[KeybindingContext::Chat]),
        ResolveOutcome::NoMatch,
    );
}

#[test]
fn escape_cancels_pending_chord() {
    let bindings = vec![binding(
        "ctrl+k ctrl+s",
        KeybindingAction::ChatStash,
        KeybindingContext::Chat,
    )];
    let mut resolver = ChordResolver::new(&bindings);
    let first = parse_combo("ctrl+k").unwrap();
    let escape = parse_combo("escape").unwrap();
    assert_eq!(
        resolver.feed(&first, &[KeybindingContext::Chat]),
        ResolveOutcome::Pending,
    );
    assert_eq!(
        resolver.feed(&escape, &[KeybindingContext::Chat]),
        ResolveOutcome::ChordCancelled,
    );
    assert!(!resolver.has_pending());
}

#[test]
fn unmatched_followup_returns_chord_cancelled() {
    // Distinct from `chord_breaks_on_unmatched_second_combo` (which
    // tests an older API): when a pending chord is broken, the new
    // outcome is `ChordCancelled`, not `NoMatch`.
    let bindings = vec![binding(
        "ctrl+k ctrl+s",
        KeybindingAction::ChatStash,
        KeybindingContext::Chat,
    )];
    let mut resolver = ChordResolver::new(&bindings);
    let first = parse_combo("ctrl+k").unwrap();
    let unrelated = parse_combo("ctrl+z").unwrap();
    resolver.feed(&first, &[KeybindingContext::Chat]);
    assert_eq!(
        resolver.feed(&unrelated, &[KeybindingContext::Chat]),
        ResolveOutcome::ChordCancelled,
    );
}

#[test]
fn tick_times_out_pending_chord() {
    use std::time::Duration;
    use std::time::Instant;

    let bindings = vec![binding(
        "ctrl+k ctrl+s",
        KeybindingAction::ChatStash,
        KeybindingContext::Chat,
    )];
    let mut resolver = ChordResolver::new(&bindings);
    let first = parse_combo("ctrl+k").unwrap();
    resolver.feed(&first, &[KeybindingContext::Chat]);
    assert!(resolver.has_pending());

    // Just before the timeout — no cancellation yet.
    let before = Instant::now() + Duration::from_millis(500);
    assert_eq!(resolver.tick(before), None);
    assert!(resolver.has_pending());

    // Past the 1-second window — chord cancelled.
    let after = Instant::now() + Duration::from_millis(1500);
    assert_eq!(resolver.tick(after), Some(ResolveOutcome::ChordCancelled));
    assert!(!resolver.has_pending());
}

#[test]
fn pending_combos_are_exposed() {
    let bindings = vec![binding(
        "ctrl+k ctrl+s",
        KeybindingAction::ChatStash,
        KeybindingContext::Chat,
    )];
    let mut resolver = ChordResolver::new(&bindings);
    let first = parse_combo("ctrl+k").unwrap();
    resolver.feed(&first, &[KeybindingContext::Chat]);
    let pending = resolver.pending_combos();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].key, "k");
    assert!(pending[0].ctrl);
}

#[test]
fn pending_display_returns_status_bar_hint() {
    use crate::display::DisplayPlatform;

    let bindings = vec![binding(
        "ctrl+k ctrl+s",
        KeybindingAction::ChatStash,
        KeybindingContext::Chat,
    )];
    let mut resolver = ChordResolver::new(&bindings);
    assert!(resolver.pending_display(DisplayPlatform::Linux).is_none());
    let first = parse_combo("ctrl+k").unwrap();
    resolver.feed(&first, &[KeybindingContext::Chat]);
    let hint = resolver.pending_display(DisplayPlatform::Linux).unwrap();
    assert!(hint.starts_with("ctrl+k"));
    assert!(hint.ends_with(" …"));
}

#[test]
fn display_for_finds_bound_action() {
    use crate::display::DisplayPlatform;

    let bindings = vec![binding(
        "ctrl+a",
        KeybindingAction::ChatSubmit,
        KeybindingContext::Chat,
    )];
    let resolver = ChordResolver::new(&bindings);
    let display = resolver
        .display_for(
            &KeybindingAction::ChatSubmit,
            &[KeybindingContext::Chat],
            DisplayPlatform::Linux,
        )
        .unwrap();
    assert_eq!(display, "ctrl+a");
}

#[test]
fn display_for_returns_none_for_unbound_action() {
    use crate::display::DisplayPlatform;

    let resolver = ChordResolver::new(&[]);
    assert!(
        resolver
            .display_for(
                &KeybindingAction::ChatSubmit,
                &[KeybindingContext::Chat],
                DisplayPlatform::Linux,
            )
            .is_none()
    );
}

#[test]
fn display_for_prefers_last_wins_user_override() {
    use crate::display::DisplayPlatform;

    let bindings = vec![
        binding(
            "ctrl+a",
            KeybindingAction::ChatSubmit,
            KeybindingContext::Chat,
        ),
        // User override defined later → should win over the default.
        binding(
            "ctrl+b",
            KeybindingAction::ChatSubmit,
            KeybindingContext::Chat,
        ),
    ];
    let resolver = ChordResolver::new(&bindings);
    let display = resolver
        .display_for(
            &KeybindingAction::ChatSubmit,
            &[KeybindingContext::Chat],
            DisplayPlatform::Linux,
        )
        .unwrap();
    assert_eq!(display, "ctrl+b");
}
