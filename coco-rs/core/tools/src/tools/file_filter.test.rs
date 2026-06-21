use super::*;
use std::path::Path;

// -----------------------------------------------------------------------
// compile_glob_matcher — rg --glob semantics as a per-file matcher
// -----------------------------------------------------------------------

#[test]
fn bare_basename_matches_at_any_depth() {
    let root = Path::new("/repo");
    let m = compile_glob_matcher(root, &["Cargo.toml"]).unwrap();
    assert!(m.matched(Path::new("Cargo.toml"), false).is_whitelist());
    assert!(
        m.matched(Path::new("app/cli/Cargo.toml"), false)
            .is_whitelist()
    );
    assert!(
        !m.matched(Path::new("app/cli/main.rs"), false)
            .is_whitelist()
    );
}

#[test]
fn extension_glob_matches_at_any_depth() {
    let m = compile_glob_matcher(Path::new("/repo"), &["*.rs"]).unwrap();
    assert!(m.matched(Path::new("a.rs"), false).is_whitelist());
    assert!(m.matched(Path::new("src/b.rs"), false).is_whitelist());
    assert!(!m.matched(Path::new("a.txt"), false).is_whitelist());
}

#[test]
fn path_segment_glob_is_root_relative() {
    let m = compile_glob_matcher(Path::new("/repo"), &["subdir/*.rs"]).unwrap();
    assert!(m.matched(Path::new("subdir/a.rs"), false).is_whitelist());
    assert!(!m.matched(Path::new("a.rs"), false).is_whitelist());
    assert!(!m.matched(Path::new("other/a.rs"), false).is_whitelist());
}

#[test]
fn brace_alternation_supported() {
    let m = compile_glob_matcher(Path::new("/repo"), &["*.{rs,txt}"]).unwrap();
    assert!(m.matched(Path::new("a.rs"), false).is_whitelist());
    assert!(m.matched(Path::new("a.txt"), false).is_whitelist());
    assert!(!m.matched(Path::new("a.md"), false).is_whitelist());
}

#[test]
fn multiple_patterns_union() {
    let m = compile_glob_matcher(
        Path::new("/repo"),
        &["*.rs".to_string(), "*.md".to_string()],
    )
    .unwrap();
    assert!(m.matched(Path::new("a.rs"), false).is_whitelist());
    assert!(m.matched(Path::new("a.md"), false).is_whitelist());
    assert!(!m.matched(Path::new("a.txt"), false).is_whitelist());
}

#[test]
fn invalid_pattern_errors() {
    assert!(compile_glob_matcher(Path::new("/repo"), &["[invalid"]).is_err());
}

// -----------------------------------------------------------------------
// build_exclusion_override — negatives-only (prunes, never whitelists)
// -----------------------------------------------------------------------

#[test]
fn exclusion_override_has_no_whitelist() {
    // A negatives-only override must never whitelist — otherwise it would
    // outrank ignore files (including .agentignore).
    let o = build_exclusion_override(Path::new("/repo"), VCS, &["*.env".to_string()]).unwrap();
    assert_eq!(o.num_whitelists(), 0);
    // VCS dir is excluded.
    assert!(o.matched(Path::new(".git"), true).is_ignore());
    // read-ignore pattern excluded at any depth.
    assert!(o.matched(Path::new("config/secret.env"), false).is_ignore());
    // unrelated files are untouched (None, not ignore) so the walk keeps them.
    assert!(o.matched(Path::new("src/main.rs"), false).is_none());
}

#[test]
fn exclusion_override_empty_is_noop() {
    let o = build_exclusion_override(Path::new("/repo"), &[], &[]).unwrap();
    assert!(o.is_empty());
    assert!(o.matched(Path::new("anything"), false).is_none());
}

#[test]
fn read_ignore_negative_globs_anchoring() {
    // `/`-anchored stays anchored; relative is prefixed with `**/`.
    let globs =
        read_ignore_negative_globs(&["/build".to_string(), "*.env".to_string(), "secrets".into()]);
    assert_eq!(globs, vec!["!/build", "!**/*.env", "!**/secrets"]);
}

const VCS: &[&str] = &["!.git", "!.svn"];
