use super::*;

#[test]
fn git_with_sha() {
    let v = calculate_plugin_version(VersionSource::Git {
        sha: Some("abcdef1234567890"),
        ref_: Some("main"),
    });
    assert_eq!(v, "abcdef123456");
}

#[test]
fn git_with_only_ref() {
    let v = calculate_plugin_version(VersionSource::Git {
        sha: None,
        ref_: Some("v1"),
    });
    assert_eq!(v, "ref-v1");
}

#[test]
fn package_version_passthrough() {
    let v = calculate_plugin_version(VersionSource::Package { version: "1.2.3" });
    assert_eq!(v, "1.2.3");
}

#[test]
fn local_content_hash_stable() {
    let bytes = b"hello world";
    let v1 = calculate_plugin_version(VersionSource::LocalOrUrl {
        manifest_bytes: bytes,
    });
    let v2 = calculate_plugin_version(VersionSource::LocalOrUrl {
        manifest_bytes: bytes,
    });
    assert_eq!(v1, v2);
    assert_eq!(v1.len(), 12);
}

#[test]
fn versioned_path_layout() {
    let p = versioned_cache_path(std::path::Path::new("/cache"), "foo", "1.0.0");
    assert_eq!(p, std::path::Path::new("/cache/foo/1.0.0"));
}
