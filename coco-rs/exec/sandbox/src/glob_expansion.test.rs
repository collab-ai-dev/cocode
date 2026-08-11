use super::*;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

fn limits(depth: usize, matches: usize, entries: usize) -> ExpansionLimits {
    ExpansionLimits {
        depth,
        matches,
        entries,
    }
}

#[test]
fn looks_like_glob_detects_metacharacters() {
    assert!(looks_like_glob("**/*.env"));
    assert!(looks_like_glob("foo?bar"));
    assert!(looks_like_glob("[abc]"));
    assert!(!looks_like_glob("/abs/path/file.env"));
    assert!(!looks_like_glob("relative/path"));
}

#[test]
fn no_globs_is_empty_but_zero_depth_fails_closed() {
    let tmp = TempDir::new().unwrap();
    let roots = vec![tmp.path().to_path_buf()];
    assert_eq!(expand(&roots, &[], 0).unwrap(), Vec::<PathBuf>::new());
    assert!(expand(&roots, &["*.env".to_string()], 0).is_err());
}

#[test]
fn relative_glob_without_an_anchor_fails_closed() {
    let error = expand(&[], &["**/*.env".to_string()], 3)
        .expect_err("a relative deny must not silently match nothing");

    assert!(error.to_string().contains("has no writable root"));
}

#[test]
fn expand_matches_leaf_pattern_under_root() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::write(root.join(".env"), "SECRET=1").unwrap();
    std::fs::write(root.join("not_secret.txt"), "ok").unwrap();

    let matches = expand(&[root.to_path_buf()], &["*.env".to_string()], 3).unwrap();

    assert_eq!(matches, vec![root.join(".env")]);
}

#[test]
fn invalid_pattern_rejects_the_whole_expansion() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("a.txt"), "x").unwrap();

    let error = expand(
        &[tmp.path().to_path_buf()],
        &["[unclosed".to_string(), "*.txt".to_string()],
        3,
    )
    .expect_err("a malformed deny must not be skipped");

    assert!(error.to_string().contains("deny-read glob"));
}

#[test]
fn non_portable_glob_syntax_is_rejected_on_both_backends() {
    for pattern in [
        "file?.env",
        "[a-z]*.env",
        "{a,b}*.env",
        "a**b",
        "../*.env",
        "nested//*.env",
    ] {
        let error = compile_deny_glob(pattern).expect_err("syntax must fail closed");
        assert!(!error.to_string().is_empty(), "{pattern}");
    }
}

#[test]
fn double_star_matches_root_and_nested_files() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("nested")).unwrap();
    std::fs::write(tmp.path().join("root.env"), "secret").unwrap();
    std::fs::write(tmp.path().join("nested/deep.env"), "secret").unwrap();

    let matches = expand(&[tmp.path().to_path_buf()], &["**/*.env".to_string()], 3).unwrap();

    assert_eq!(
        matches,
        vec![
            tmp.path().join("nested/deep.env"),
            tmp.path().join("root.env")
        ]
    );
}

#[test]
fn reaching_the_depth_cap_fails_closed() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("a/b/c")).unwrap();

    let error = expand(&[tmp.path().to_path_buf()], &["**/*.env".to_string()], 2)
        .expect_err("a capped directory can hide a deeper match");

    assert!(error.to_string().contains("deeper matches could be hidden"));
}

#[test]
fn literal_prefix_avoids_unrelated_tree_work() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    for index in 0..20 {
        std::fs::create_dir_all(root.join(format!("unrelated-{index}/nested"))).unwrap();
    }
    std::fs::create_dir_all(root.join("secrets")).unwrap();
    std::fs::write(root.join("secrets/key.pem"), "secret").unwrap();

    let matches = expand_with_limits(
        &[root.to_path_buf()],
        &["secrets/*.pem".to_string()],
        limits(3, 10, 3),
    )
    .expect("the scan starts at the literal secrets prefix");

    assert_eq!(matches, vec![root.join("secrets/key.pem")]);
}

#[test]
fn entry_and_match_caps_fail_closed() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    for name in ["a.env", "b.env", "c.env"] {
        std::fs::write(root.join(name), "secret").unwrap();
    }

    let entry_error = expand_with_limits(
        &[root.to_path_buf()],
        &["*.env".to_string()],
        limits(3, 10, 2),
    )
    .expect_err("entry budget must be enforced");
    assert!(entry_error.to_string().contains("visited over 2 entries"));

    let match_error = expand_with_limits(
        &[root.to_path_buf()],
        &["*.env".to_string()],
        limits(3, 2, 10),
    )
    .expect_err("match budget must be enforced");
    assert!(match_error.to_string().contains("matched over 2 paths"));
}

#[test]
fn absolute_glob_uses_its_own_literal_root() {
    let tmp = TempDir::new().unwrap();
    let secrets = tmp.path().join("secrets");
    std::fs::create_dir_all(&secrets).unwrap();
    std::fs::write(secrets.join("key.pem"), "secret").unwrap();
    let pattern = format!("{}/*.pem", secrets.display());

    let matches = expand(&[], &[pattern], 3).unwrap();

    assert_eq!(matches, vec![secrets.join("key.pem")]);
}

#[cfg(unix)]
#[test]
fn absolute_glob_follows_its_named_symlink_root() {
    use std::os::unix::fs::symlink;

    let tmp = TempDir::new().unwrap();
    let actual = tmp.path().join("actual");
    let alias = tmp.path().join("alias");
    std::fs::create_dir(&actual).unwrap();
    std::fs::write(actual.join("key.pem"), "secret").unwrap();
    symlink(&actual, &alias).unwrap();
    let pattern = format!("{}/*.pem", alias.display());

    let matches = expand(&[], &[pattern], 3).unwrap();

    assert!(matches.contains(&alias.join("key.pem")));
    assert!(matches.contains(&actual.join("key.pem")));
}

#[test]
fn seatbelt_regex_covers_root_and_future_nested_matches() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();

    let filters =
        seatbelt_regex_filters(std::slice::from_ref(&root), &["**/*.env".to_string()]).unwrap();
    let root = escape_regex_literal(root.to_str().unwrap());
    let expected = format!(r#"(regex #"^{root}/(.*/)?[^/]*\.env$")"#);

    assert!(filters.contains(&expected));
}

#[test]
fn seatbelt_regex_covers_private_firmlink_alias() {
    let filters = seatbelt_regex_filters(&[], &["/tmp/secrets/*.pem".to_string()]).unwrap();

    assert!(
        filters
            .iter()
            .any(|filter| filter.contains(r#"^/tmp/secrets/[^/]*\.pem$"#))
    );
    assert!(
        filters
            .iter()
            .any(|filter| filter.contains(r#"^/private/tmp/secrets/[^/]*\.pem$"#))
    );
}

#[test]
fn seatbelt_regex_and_globset_match_the_same_portable_subset() {
    let segments = [
        "a", "x.y", "*", "?", "**", "[abc]", "[a-z]", "[!a]", "[^a]", "[.]", "[*]", "*]",
    ];
    let candidates = [
        "a",
        "ab",
        "x",
        "x.y",
        "xay",
        "^",
        ".",
        "-",
        "*",
        "b",
        "a/b",
        "ab/cd",
        "sub/a",
        "sub/dir/a",
        ".env",
        "sub/.env",
        "k.pem",
        "foo.env",
        ".envrc",
        "secrets/x",
        "]",
        "a]",
        "é",
    ];

    let patterns = segments.iter().flat_map(|first| {
        std::iter::once((*first).to_string()).chain(
            segments
                .iter()
                .map(move |second| format!("{first}/{second}")),
        )
    });
    for pattern in patterns {
        let Ok(glob) = compile_deny_glob(&pattern) else {
            continue;
        };
        let filters =
            seatbelt_regex_filters(&[PathBuf::from("/ws")], std::slice::from_ref(&pattern))
                .expect("validated pattern must translate");
        assert_eq!(filters.len(), 1, "unexpected aliases for {pattern:?}");
        let regex_source = filters[0]
            .strip_prefix("(regex #\"")
            .and_then(|filter| filter.strip_suffix("\")"))
            .expect("Seatbelt regex wrapper");
        let regex = regex::Regex::new(regex_source)
            .unwrap_or_else(|error| panic!("invalid regex for {pattern:?}: {error}"));
        let matcher = glob.compile_matcher();

        for candidate in candidates {
            assert_eq!(
                regex.is_match(&format!("/ws/{candidate}")),
                matcher.is_match(candidate),
                "backend drift for pattern {pattern:?} and path {candidate:?}"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn symlink_match_also_denies_its_canonical_target() {
    use std::os::unix::fs::symlink;

    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("actual.pem");
    let link = tmp.path().join("secret.pem");
    std::fs::write(&target, "secret").unwrap();
    symlink(&target, &link).unwrap();

    let matches = expand(&[tmp.path().to_path_buf()], &["secret.*".to_string()], 3).unwrap();

    assert!(matches.contains(&link));
    assert!(matches.contains(&target));
}

#[cfg(unix)]
#[test]
fn unresolved_symlink_match_fails_closed() {
    use std::os::unix::fs::symlink;

    let tmp = TempDir::new().unwrap();
    symlink(
        tmp.path().join("missing.pem"),
        tmp.path().join("secret.pem"),
    )
    .unwrap();

    let error = expand(&[tmp.path().to_path_buf()], &["*.pem".to_string()], 3)
        .expect_err("an unresolved alias could leave a readable target unmasked later");

    assert!(error.to_string().contains("could not resolve"));
}

#[cfg(unix)]
#[test]
fn non_utf8_path_in_a_scanned_tree_fails_closed() {
    use std::os::unix::ffi::OsStringExt;

    let tmp = TempDir::new().unwrap();
    let invalid_name = std::ffi::OsString::from_vec(vec![b's', 0xff]);
    std::fs::write(tmp.path().join(invalid_name), "secret").unwrap();

    let error = expand(&[tmp.path().to_path_buf()], &["*".to_string()], 3)
        .expect_err("a path that cannot be passed losslessly to bwrap must fail");

    assert!(error.to_string().contains("non-UTF-8 path"));
}

#[test]
fn resolve_config_merges_and_clears_globs() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("secret.env"), "secret").unwrap();
    let mut config = SandboxConfig::default();
    config.writable_roots = vec![crate::config::WritableRoot::unprotected(tmp.path())];
    config.denied_read_globs = vec!["*.env".to_string()];
    config.glob_scan_max_depth = 3;

    let resolved = resolve_config(&config).unwrap();

    assert!(resolved.denied_read_globs.is_empty());
    assert!(
        resolved
            .denied_read_paths
            .contains(&tmp.path().join("secret.env"))
    );
    assert!(
        config.denied_read_paths.is_empty(),
        "input remains immutable"
    );
}
