use super::*;

#[test]
fn ascii_basic_indices() {
    let (idx, score) = match fuzzy_match("hello", "hl") {
        Some(v) => v,
        None => panic!("expected a match"),
    };
    assert_eq!(idx, vec![0, 2]);
    // 'h' at 0, 'l' at 2 -> window 1; start-of-string bonus applies (-100)
    assert_eq!(score, -99);
}

#[test]
fn unicode_dotted_i_istanbul_highlighting() {
    let (idx, score) = match fuzzy_match("İstanbul", "is") {
        Some(v) => v,
        None => panic!("expected a match"),
    };
    assert_eq!(idx, vec![0, 1]);
    // Matches at lowered positions 0 and 2 -> window 1; start-of-string bonus applies
    assert_eq!(score, -99);
}

#[test]
fn unicode_german_sharp_s_casefold() {
    assert!(fuzzy_match("straße", "strasse").is_none());
}

#[test]
fn prefer_contiguous_match_over_spread() {
    let (_idx_a, score_a) = match fuzzy_match("abc", "abc") {
        Some(v) => v,
        None => panic!("expected a match"),
    };
    let (_idx_b, score_b) = match fuzzy_match("a-b-c", "abc") {
        Some(v) => v,
        None => panic!("expected a match"),
    };
    // Contiguous window -> 0; start-of-string bonus -> -100
    assert_eq!(score_a, -100);
    // Spread over 5 chars for 3-letter needle -> window 2; with bonus -> -98
    assert_eq!(score_b, -98);
    assert!(score_a < score_b);
}

#[test]
fn start_of_string_bonus_applies() {
    let (_idx_a, score_a) = match fuzzy_match("file_name", "file") {
        Some(v) => v,
        None => panic!("expected a match"),
    };
    let (_idx_b, score_b) = match fuzzy_match("my_file_name", "file") {
        Some(v) => v,
        None => panic!("expected a match"),
    };
    // Start-of-string contiguous -> window 0; bonus -> -100
    assert_eq!(score_a, -100);
    // Non-prefix contiguous -> window 0; no bonus -> 0
    assert_eq!(score_b, 0);
    assert!(score_a < score_b);
}

#[test]
fn empty_needle_matches_with_max_score_and_no_indices() {
    let (idx, score) = match fuzzy_match("anything", "") {
        Some(v) => v,
        None => panic!("empty needle should match"),
    };
    assert!(idx.is_empty());
    assert_eq!(score, i32::MAX);
}

#[test]
fn case_insensitive_matching_basic() {
    let (idx, score) = match fuzzy_match("FooBar", "foO") {
        Some(v) => v,
        None => panic!("expected a match"),
    };
    assert_eq!(idx, vec![0, 1, 2]);
    // Contiguous prefix match (case-insensitive) -> window 0 with bonus
    assert_eq!(score, -100);
}

#[test]
fn indices_are_deduped_for_multichar_lowercase_expansion() {
    let needle = "\u{0069}\u{0307}"; // "i" + combining dot above
    let (idx, score) = match fuzzy_match("İ", needle) {
        Some(v) => v,
        None => panic!("expected a match"),
    };
    assert_eq!(idx, vec![0]);
    // Lowercasing 'İ' expands to two chars; contiguous prefix -> window 0 with bonus
    assert_eq!(score, -100);
}
