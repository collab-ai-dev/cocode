use std::path::Path;

use super::*;

#[test]
fn parse_version_plain() {
    assert_eq!(
        parse_version("rtk 0.42.4"),
        Some(RtkVersion {
            major: 0,
            minor: 42,
            patch: 4
        })
    );
}

#[test]
fn parse_version_pre_release_suffix_dropped() {
    // rr-rtk pre-release tags compare by their base triple.
    assert_eq!(
        parse_version("rr-rtk 0.42.3-rr.2"),
        Some(RtkVersion {
            major: 0,
            minor: 42,
            patch: 3
        })
    );
}

#[test]
fn parse_version_leading_v_and_build_metadata() {
    assert_eq!(
        parse_version("rtk v1.5.0+build.7"),
        Some(RtkVersion {
            major: 1,
            minor: 5,
            patch: 0
        })
    );
}

#[test]
fn parse_version_rejects_incomplete_and_junk() {
    assert_eq!(parse_version("rtk"), None);
    assert_eq!(parse_version("0.42"), None);
    assert_eq!(parse_version("1.2.3.4"), None);
    assert_eq!(parse_version("not a version"), None);
}

#[test]
fn version_ordering_and_min_gate() {
    let old = RtkVersion {
        major: 0,
        minor: 22,
        patch: 9,
    };
    let ok = RtkVersion {
        major: 0,
        minor: 23,
        patch: 0,
    };
    let newer = RtkVersion {
        major: 0,
        minor: 42,
        patch: 4,
    };
    assert!(old < MIN_VERSION);
    assert!(ok >= MIN_VERSION);
    assert!(newer > ok);
}

#[test]
fn flavor_inferred_from_binary_stem() {
    assert_eq!(flavor_from_path(Path::new("/usr/bin/rtk")), RtkFlavor::Rtk);
    assert_eq!(
        flavor_from_path(Path::new("/home/u/.cargo/bin/rr-rtk")),
        RtkFlavor::RrRtk
    );
    // Unknown stems default to the plain flavor (no fixup).
    assert_eq!(
        flavor_from_path(Path::new("/opt/custom-rtk-wrapper")),
        RtkFlavor::Rtk
    );
}

#[test]
fn resolve_path_prefers_configured_binary() {
    let config = RtkConfig {
        binary_path: Some("/opt/rr-rtk".to_string()),
        ..Default::default()
    };
    let (path, flavor) = resolve_path(&config).expect("configured path resolves");
    assert_eq!(path, PathBuf::from("/opt/rr-rtk"));
    assert_eq!(flavor, RtkFlavor::RrRtk);
}
