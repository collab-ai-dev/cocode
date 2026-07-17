use super::*;
use coco_types::{
    AgentSkillLifecycleWire, JourneyNodeBodyWire, JourneyNodeWire, JourneyStatsWire,
    SkillTelemetryWire,
};
use pretty_assertions::assert_eq;

fn skill_node(title: &str, path: &str) -> JourneyNodeWire {
    JourneyNodeWire {
        title: title.into(),
        description: String::new(),
        first_seen_ms: 0,
        last_activity_ms: 0,
        date_label: "1 Jan".into(),
        body: JourneyNodeBodyWire::AgentSkill {
            path: path.into(),
            lifecycle: AgentSkillLifecycleWire::Learning {
                progress: coco_types::SkillQuarantineWire {
                    invocations: 0,
                    required: 5,
                },
            },
            telemetry: SkillTelemetryWire::default(),
        },
        history: Vec::new(),
    }
}

fn payload(nodes: Vec<JourneyNodeWire>) -> JourneyDialogPayload {
    JourneyDialogPayload {
        nodes,
        buckets: Vec::new(),
        stats: JourneyStatsWire::default(),
    }
}

#[test]
fn test_from_wire_starts_in_list_at_top() {
    let state = JourneyState::from_wire(payload(vec![skill_node("a", "/a")]));
    assert_eq!(state.selected, 0);
    assert_eq!(state.mode, JourneyMode::List);
}

#[test]
fn test_nav_clamps_no_wrap() {
    let mut state =
        JourneyState::from_wire(payload(vec![skill_node("a", "/a"), skill_node("b", "/b")]));
    state.nav(-1);
    assert_eq!(state.selected, 0);
    state.nav(1);
    assert_eq!(state.selected, 1);
    state.nav(1);
    assert_eq!(state.selected, 1, "no wrap past end");
}

#[test]
fn test_refresh_preserves_selection_by_identity() {
    let mut state = JourneyState::from_wire(payload(vec![
        skill_node("a", "/a"),
        skill_node("b", "/b"),
        skill_node("c", "/c"),
    ]));
    state.selected = 2; // node "/c"
    // Refresh drops "/a" — "/c" is now at index 1.
    state.refresh_from_wire(payload(vec![skill_node("b", "/b"), skill_node("c", "/c")]));
    assert_eq!(state.selected_node().unwrap().title, "c");
}

#[test]
fn test_refresh_clamps_when_identity_gone() {
    let mut state =
        JourneyState::from_wire(payload(vec![skill_node("a", "/a"), skill_node("b", "/b")]));
    state.selected = 1;
    // "/b" removed; selection clamps into range.
    state.refresh_from_wire(payload(vec![skill_node("a", "/a")]));
    assert_eq!(state.selected, 0);
}

#[test]
fn test_refresh_resets_mode_to_list() {
    let mut state = JourneyState::from_wire(payload(vec![skill_node("a", "/a")]));
    state.mode = JourneyMode::Detail;
    state.refresh_from_wire(payload(vec![skill_node("a", "/a")]));
    assert_eq!(state.mode, JourneyMode::List);
}
