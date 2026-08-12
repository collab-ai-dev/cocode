#[test]
fn missing_bundle_is_ignored() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("missing.pem");
    // SAFETY: this integration-test binary has one test and resolves the
    // process-wide cache only after setting the variable.
    unsafe {
        std::env::set_var(coco_utils_extra_ca::ENV_COCO_EXTRA_CA_BUNDLE, missing);
    }

    assert!(coco_utils_extra_ca::extra_root_ders().is_empty());
    coco_utils_extra_ca::client_builder()
        .build()
        .expect("client after missing optional bundle");
}
