use super::*;

#[test]
fn test_default_filter() {
    let filter = FileFilter::new(Path::new("/project"), &[], &[], &[], &[]);

    assert!(filter.should_include(Path::new("/project/src/main.rs")));
    assert!(filter.should_include(Path::new("/project/package.json")));
    assert!(!filter.should_include(Path::new("/project/image.png")));
    assert!(!filter.summary().has_filters());
}

#[test]
fn test_include_dirs() {
    let filter = FileFilter::new(
        Path::new("/project"),
        &["src".to_string(), "lib".to_string()],
        &[],
        &[],
        &[],
    );

    assert!(filter.should_include(Path::new("/project/src/main.rs")));
    assert!(filter.should_include(Path::new("/project/lib/utils.rs")));
    assert!(!filter.should_include(Path::new("/project/tests/test.rs")));
}

#[test]
fn test_exclude_dirs() {
    let filter = FileFilter::new(
        Path::new("/project"),
        &[],
        &["vendor".to_string(), "node_modules".to_string()],
        &[],
        &[],
    );

    assert!(filter.should_include(Path::new("/project/src/main.rs")));
    assert!(!filter.should_include(Path::new("/project/vendor/lib.rs")));
    assert!(!filter.should_include(Path::new("/project/node_modules/pkg/index.js")));
}

#[test]
fn test_include_extensions() {
    let filter = FileFilter::new(
        Path::new("/project"),
        &[],
        &[],
        &["rs".to_string(), "py".to_string()],
        &[],
    );

    assert!(filter.should_include(Path::new("/project/main.rs")));
    assert!(filter.should_include(Path::new("/project/script.py")));
    assert!(!filter.should_include(Path::new("/project/app.ts")));
    assert!(!filter.should_include(Path::new("/project/config.json")));
}

#[test]
fn test_exclude_extensions() {
    let filter = FileFilter::new(
        Path::new("/project"),
        &[],
        &[],
        &[],
        &["test.ts".to_string(), "spec.js".to_string()],
    );

    assert!(filter.should_include(Path::new("/project/app.ts")));
    assert!(!filter.should_include(Path::new("/project/app.test.ts")));
    assert!(!filter.should_include(Path::new("/project/user.spec.js")));
}

#[test]
fn test_combined_filters() {
    let filter = FileFilter::new(
        Path::new("/project"),
        &["src".to_string()],
        &["src/vendor".to_string()],
        &["ts".to_string()],
        &["test.ts".to_string()],
    );

    assert!(filter.should_include(Path::new("/project/src/app.ts")));
    assert!(!filter.should_include(Path::new("/project/src/app.test.ts")));
    assert!(!filter.should_include(Path::new("/project/src/vendor/lib.ts")));
    assert!(!filter.should_include(Path::new("/project/lib/util.ts")));
    assert!(!filter.should_include(Path::new("/project/src/main.rs")));
}

#[test]
fn test_case_insensitive() {
    let filter = FileFilter::new(
        Path::new("/project"),
        &[],
        &[],
        &["RS".to_string()],
        &["TEST.RS".to_string()],
    );

    assert!(filter.should_include(Path::new("/project/main.rs")));
    assert!(filter.should_include(Path::new("/project/main.RS")));
    assert!(!filter.should_include(Path::new("/project/app.test.rs")));
}

#[test]
fn test_filter_summary() {
    let filter = FileFilter::new(
        Path::new("/project"),
        &["src".to_string()],
        &["vendor".to_string()],
        &["rs".to_string()],
        &["test.rs".to_string()],
    );

    let summary = filter.summary();
    assert!(summary.has_filters());
    assert!(!summary.to_display_string().is_empty());
}

#[test]
fn test_is_default_text_file() {
    assert!(is_default_text_file(Path::new("main.rs")));
    assert!(is_default_text_file(Path::new("package.json")));
    assert!(is_default_text_file(Path::new("README.md")));
    assert!(!is_default_text_file(Path::new("image.png")));
    assert!(!is_default_text_file(Path::new("binary.exe")));
}
