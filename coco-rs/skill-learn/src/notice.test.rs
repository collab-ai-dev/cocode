use super::*;
use pretty_assertions::assert_eq;

#[test]
fn test_push_drain_roundtrip() {
    let inbox = SkillLearnInbox::new();
    assert!(inbox.is_empty());
    inbox.push(SkillLearnNotice {
        name: "a".into(),
        verb: SkillLearnVerb::Learned,
    });
    inbox.push(SkillLearnNotice {
        name: "b".into(),
        verb: SkillLearnVerb::Updated,
    });
    assert_eq!(inbox.len(), 2);
    let drained = inbox.drain();
    assert_eq!(drained.len(), 2);
    assert!(inbox.is_empty(), "drain clears the inbox");
}

#[test]
fn test_drain_dedups_learned_over_updated() {
    let inbox = SkillLearnInbox::new();
    inbox.push(SkillLearnNotice {
        name: "x".into(),
        verb: SkillLearnVerb::Updated,
    });
    inbox.push(SkillLearnNotice {
        name: "x".into(),
        verb: SkillLearnVerb::Learned,
    });
    let drained = inbox.drain();
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].verb, SkillLearnVerb::Learned);
}

#[test]
fn test_verb_as_str() {
    assert_eq!(SkillLearnVerb::Learned.as_str(), "Learned");
    assert_eq!(SkillLearnVerb::Updated.as_str(), "Improved");
}
