#[test]
fn bundle_without_certificates_is_ignored() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("invalid.pem");
    std::fs::write(&path, b"not a PEM certificate").expect("write fixture");

    // SAFETY: this integration-test binary has one test and resolves the
    // process-wide cache only after setting the variable.
    unsafe {
        std::env::set_var(coco_utils_extra_ca::ENV_COCO_EXTRA_CA_BUNDLE, &path);
    }

    assert!(coco_utils_extra_ca::extra_root_ders().is_empty());
    assert!(coco_utils_extra_ca::client_builder().build().is_ok());
}
