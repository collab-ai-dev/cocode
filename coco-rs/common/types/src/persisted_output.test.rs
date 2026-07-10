use super::*;

#[test]
fn prefix_reference_is_both_persisted_and_pointer_bearing() {
    let s = "<persisted-output>\nFull output saved to: /s/x.txt\n</persisted-output>";
    assert!(is_content_already_persisted(s));
    assert!(is_pointer_bearing(s));
}

#[test]
fn windowed_suffix_footer_is_pointer_bearing_but_not_persisted() {
    let s = "head text\n\n[... middle omitted ...]\n\ntail\n\n<persisted-output>\n...\n</persisted-output>";
    assert!(!is_content_already_persisted(s));
    assert!(is_pointer_bearing(s));
}

#[test]
fn plain_content_matches_neither() {
    assert!(!is_content_already_persisted("just output"));
    assert!(!is_pointer_bearing("just output"));
}

#[test]
fn trailing_whitespace_tolerated() {
    let s = "tail\n\n</persisted-output>\n  \n";
    assert!(is_pointer_bearing(s));
}

#[test]
fn pointer_footer_strips_windowed_body() {
    let s = "big head text\n\n[... omitted ...]\n\ntail\n\n<persisted-output>\nFull text saved to: /s/x.txt\n</persisted-output>";
    let footer = pointer_footer(s).expect("windowed content has a strippable footer");
    assert!(footer.starts_with(PERSISTED_OUTPUT_TAG));
    assert!(footer.trim_end().ends_with(PERSISTED_OUTPUT_CLOSING_TAG));
    assert!(footer.contains("/s/x.txt"));
    assert!(!footer.contains("big head text"));
}

#[test]
fn pointer_footer_none_for_minimal_or_plain_content() {
    // Prefix-form reference: already minimal.
    assert!(pointer_footer("<persisted-output>\nsaved\n</persisted-output>").is_none());
    // Leading whitespace only: still minimal.
    assert!(pointer_footer("\n  <persisted-output>\nsaved\n</persisted-output>\n").is_none());
    // Plain content: nothing to strip.
    assert!(pointer_footer("just output").is_none());
}
