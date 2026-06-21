use super::SPINNER_VERBS;
use super::pick_verb;
use pretty_assertions::assert_eq;

#[test]
fn lexicon_size_matches_ts_source() {
    // Byte-faithful with spinnerVerbs.ts SPINNER_VERBS array.
    assert_eq!(SPINNER_VERBS.len(), 186);
}

#[test]
fn lexicon_contains_canonical_verbs() {
    assert!(SPINNER_VERBS.contains(&"Pondering"));
    assert!(SPINNER_VERBS.contains(&"Honking"));
    assert!(SPINNER_VERBS.contains(&"Thinking"));
    assert!(SPINNER_VERBS.contains(&"Working"));
    assert!(SPINNER_VERBS.contains(&"Accomplishing"));
}

#[test]
fn pick_verb_is_deterministic() {
    let v1 = pick_verb(42);
    let v2 = pick_verb(42);
    assert_eq!(v1, v2);
}

#[test]
fn pick_verb_differs_across_seeds() {
    // Two arbitrary seeds chosen to land on different indices given
    // 186 entries. (42 % 186 = 42, 100 % 186 = 100 — distinct.)
    assert_ne!(pick_verb(42), pick_verb(100));
}

#[test]
fn pick_verb_never_panics_at_max_seed() {
    let _ = pick_verb(u64::MAX);
}
