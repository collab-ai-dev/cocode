use super::generate_template;
use crate::KeybindingsConfig;

#[test]
fn template_is_valid_json_and_round_trips() {
    let content = generate_template().unwrap();
    assert!(content.ends_with('\n'));
    let parsed = KeybindingsConfig::from_json(&content).expect("template parses");
    assert!(!parsed.bindings.is_empty());
    assert!(parsed.schema.is_some());
    assert!(parsed.docs.is_some());
}

#[test]
fn template_excludes_non_rebindable_chords() {
    use crate::reserved::NON_REBINDABLE;
    use crate::reserved::normalize_key_for_comparison;

    let content = generate_template().unwrap();
    let parsed = KeybindingsConfig::from_json(&content).unwrap();
    let banned: Vec<String> = NON_REBINDABLE
        .iter()
        .map(|r| normalize_key_for_comparison(r.key))
        .collect();
    for block in &parsed.bindings {
        for chord in block.bindings.keys() {
            let canonical = normalize_key_for_comparison(chord);
            assert!(
                !banned.contains(&canonical),
                "non-rebindable chord {chord:?} should not appear in template",
            );
        }
    }
}

#[test]
fn template_keeps_chord_chat_kill_agents() {
    let content = generate_template().unwrap();
    assert!(
        content.contains("ctrl+x ctrl+k"),
        "non-reserved chord must remain after filtering",
    );
}
