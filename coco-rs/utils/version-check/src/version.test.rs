use super::is_newer;
use super::is_source_build;

#[test]
fn newer_patch_minor_and_major_are_all_detected() {
    assert!(is_newer("0.1.2", "0.1.1"));
    assert!(is_newer("0.2.0", "0.1.9"));
    assert!(is_newer("1.0.0", "0.99.99"));
}

#[test]
fn same_or_older_is_not_newer() {
    assert!(!is_newer("0.1.1", "0.1.1"));
    assert!(!is_newer("0.1.0", "0.1.1"));
    assert!(!is_newer("0.9.9", "1.0.0"));
}

#[test]
fn numeric_components_compare_numerically_not_lexically() {
    // The bug a string compare ships with: "0.1.10" < "0.1.9" as text.
    assert!(is_newer("0.1.10", "0.1.9"));
    assert!(!is_newer("0.1.9", "0.1.10"));
}

#[test]
fn a_release_supersedes_its_own_prerelease() {
    assert!(is_newer("1.2.0", "1.2.0-rc.1"));
    assert!(!is_newer("1.2.0-rc.1", "1.2.0"));
    assert!(!is_newer("1.2.0-rc.1", "1.2.0-rc.1"));
}

#[test]
fn leading_v_and_surrounding_space_are_tolerated() {
    assert!(is_newer("v0.2.0", "0.1.0"));
    assert!(is_newer(" 0.2.0\n", "0.1.0"));
}

#[test]
fn missing_components_default_to_zero() {
    assert!(!is_newer("1.2", "1.2.0"));
    assert!(is_newer("1.3", "1.2.9"));
}

#[test]
fn unorderable_versions_never_claim_an_upgrade() {
    // Refusing to answer is the safe direction: a bogus "update available" is
    // worse than a missed one.
    assert!(!is_newer("banana", "0.1.0"));
    assert!(!is_newer("0.1.0", "banana"));
    assert!(!is_newer("1.2.3.4", "0.1.0"));
}

#[test]
fn source_builds_are_recognized() {
    assert!(is_source_build("0.0.0"));
    assert!(is_source_build("0.1.1-dev"));
    assert!(is_source_build("0.1.1+abcdef"));
    assert!(!is_source_build("0.1.1"));
    assert!(!is_source_build("1.2.0-rc.1"));
}
