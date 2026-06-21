use super::*;
use std::collections::HashSet;

#[test]
fn teammate_color_is_deterministic() {
    // Same id → same color on every call, with no shared state and no spawn
    // ordering dependency.
    let first = assign_teammate_color("researcher@my-team");
    assert_eq!(first, assign_teammate_color("researcher@my-team"));
    // `get` mirrors `assign` (the mapping is total).
    assert_eq!(get_teammate_color("researcher@my-team"), Some(first));
}

#[test]
fn teammate_color_is_always_in_palette() {
    for i in 0..64 {
        let color = assign_teammate_color(&format!("agent-{i}"));
        assert!(
            AGENT_COLORS.contains(&color),
            "color {color:?} not in palette"
        );
    }
}

#[test]
fn distinct_ids_spread_across_palette() {
    // Not a uniqueness guarantee (a hash can collide), but the mapping must not
    // collapse every id onto one color.
    let colors: HashSet<_> = (0..32)
        .map(|i| assign_teammate_color(&format!("agent-{i}")))
        .collect();
    assert!(
        colors.len() > 1,
        "hash should spread ids across the palette, got {colors:?}"
    );
}

#[test]
fn agent_type_color_is_deterministic() {
    let explore: coco_types::AgentTypeId = "explore".parse().expect("infallible");
    let color = get_agent_type_color(&explore);
    assert!(color.is_some());
    assert_eq!(color, get_agent_type_color(&explore));
    assert!(AGENT_COLORS.contains(&color.unwrap()));
}
