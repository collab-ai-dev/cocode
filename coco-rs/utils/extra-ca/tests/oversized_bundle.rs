use std::io::Write;

#[test]
fn oversized_bundle_is_ignored() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("oversized.pem");
    let mut file = std::fs::File::create(&path).expect("create fixture");
    let chunk = [b'x'; 64 * 1024];
    let mut remaining = coco_utils_extra_ca::MAX_EXTRA_CA_BUNDLE_BYTES + 1;
    while remaining > 0 {
        let count = usize::try_from(remaining.min(chunk.len() as u64)).expect("bounded count");
        file.write_all(&chunk[..count]).expect("write fixture");
        remaining -= count as u64;
    }
    drop(file);

    // SAFETY: this integration-test binary has one test and resolves the
    // process-wide cache only after setting the variable.
    unsafe {
        std::env::set_var(coco_utils_extra_ca::ENV_COCO_EXTRA_CA_BUNDLE, &path);
    }

    assert!(coco_utils_extra_ca::extra_root_ders().is_empty());
    assert!(coco_utils_extra_ca::client_builder().build().is_ok());
}
