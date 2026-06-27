use super::cycle_all_modes;
use super::cycle_mode;
use crate::state::AppState;
use crate::state::ModalState;

#[test]
fn cycle_mode_wraps_interactive_modes() {
    use coco_types::PermissionMode as PM;
    let member = |name: &str, mode: PM| crate::state::TeamRosterMember {
        name: name.into(),
        agent_type: "explore".into(),
        color: None,
        mode,
    };
    let mut s = AppState::new();
    s.ui.show_modal(ModalState::TeamRoster(crate::state::TeamRosterState {
        team_name: "t".into(),
        members: vec![
            member("researcher", PM::Plan),
            member("builder", PM::AcceptEdits),
        ],
        selected: 0,
    }));

    let focused = |s: &AppState| match s.ui.modal.as_ref() {
        Some(ModalState::TeamRoster(r)) => r.members[r.selected].mode,
        _ => panic!("expected TeamRoster"),
    };
    let other = |s: &AppState| match s.ui.modal.as_ref() {
        Some(ModalState::TeamRoster(r)) => r.members[1].mode,
        _ => panic!("expected TeamRoster"),
    };

    assert_eq!(
        cycle_mode(&mut s, 1),
        Some(("researcher".to_string(), PM::BypassPermissions))
    );
    assert_eq!(focused(&s), PM::BypassPermissions);
    cycle_mode(&mut s, 1);
    assert_eq!(focused(&s), PM::Default);
    cycle_mode(&mut s, -1);
    assert_eq!(focused(&s), PM::BypassPermissions);

    assert_eq!(
        other(&s),
        PM::AcceptEdits,
        "cycling member 0 must not affect member 1"
    );
}

fn roster_state(modes: &[(&str, coco_types::PermissionMode)]) -> AppState {
    let mut s = AppState::new();
    s.ui.show_modal(ModalState::TeamRoster(crate::state::TeamRosterState {
        team_name: "t".into(),
        members: modes
            .iter()
            .map(|(name, mode)| crate::state::TeamRosterMember {
                name: (*name).into(),
                agent_type: "explore".into(),
                color: None,
                mode: *mode,
            })
            .collect(),
        selected: 0,
    }));
    s
}

fn roster_modes(s: &AppState) -> Vec<coco_types::PermissionMode> {
    match s.ui.modal.as_ref() {
        Some(ModalState::TeamRoster(r)) => r.members.iter().map(|m| m.mode).collect(),
        _ => panic!("expected TeamRoster"),
    }
}

#[test]
fn cycle_all_modes_all_same_advances_in_tandem() {
    use coco_types::PermissionMode as PM;
    let mut s = roster_state(&[("a", PM::Default), ("b", PM::Default), ("c", PM::Default)]);

    let updates = cycle_all_modes(&mut s, 1);
    assert_eq!(
        updates,
        vec![
            ("a".to_string(), PM::AcceptEdits),
            ("b".to_string(), PM::AcceptEdits),
            ("c".to_string(), PM::AcceptEdits),
        ]
    );
    assert_eq!(roster_modes(&s), vec![PM::AcceptEdits; 3]);
}

#[test]
fn cycle_all_modes_divergent_resets_to_default() {
    use coco_types::PermissionMode as PM;
    let mut s = roster_state(&[("a", PM::Plan), ("b", PM::AcceptEdits), ("c", PM::Plan)]);

    let updates = cycle_all_modes(&mut s, 1);
    assert!(
        updates.iter().all(|(_, m)| *m == PM::Default),
        "got {updates:?}"
    );
    assert_eq!(roster_modes(&s), vec![PM::Default; 3]);

    let updates2 = cycle_all_modes(&mut s, 1);
    assert!(updates2.iter().all(|(_, m)| *m == PM::AcceptEdits));
}

#[test]
fn cycle_all_modes_empty_is_noop() {
    let mut s = roster_state(&[]);
    assert!(cycle_all_modes(&mut s, 1).is_empty());
}
