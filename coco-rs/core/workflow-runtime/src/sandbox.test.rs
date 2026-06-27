#[test]
fn determinism_shim_injects_both_error_messages() {
    let shim = super::determinism_shim();
    assert!(shim.contains(super::DATE_ERROR_MESSAGE));
    assert!(shim.contains(super::RANDOM_ERROR_MESSAGE));
    // The placeholders are fully substituted.
    assert!(!shim.contains("__NOW_ERR__"));
    assert!(!shim.contains("__RANDOM_ERR__"));
}
