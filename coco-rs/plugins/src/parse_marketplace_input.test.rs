//! Tests for `parse_marketplace_input`.

use super::*;
use crate::schemas::MarketplaceSource;
use std::path::PathBuf;

fn no_home() -> Option<PathBuf> {
    None
}

#[test]
fn parses_github_shorthand_without_ref() {
    let r = parse_marketplace_input("anthropics/claude-plugins-official", no_home);
    assert_eq!(
        r,
        Ok(Some(MarketplaceSource::Github {
            repo: "anthropics/claude-plugins-official".into(),
            git_ref: None,
            path: None,
            sparse_paths: None,
        }))
    );
}

#[test]
fn parses_github_shorthand_with_hash_ref() {
    let r = parse_marketplace_input("anthropics/claude-plugins-official#main", no_home);
    assert_eq!(
        r,
        Ok(Some(MarketplaceSource::Github {
            repo: "anthropics/claude-plugins-official".into(),
            git_ref: Some("main".into()),
            path: None,
            sparse_paths: None,
        }))
    );
}

#[test]
fn parses_github_shorthand_with_at_ref() {
    // Both `#ref` and `@ref` separators are accepted.
    let r = parse_marketplace_input("anthropics/claude-plugins@v1.2.3", no_home);
    assert_eq!(
        r,
        Ok(Some(MarketplaceSource::Github {
            repo: "anthropics/claude-plugins".into(),
            git_ref: Some("v1.2.3".into()),
            path: None,
            sparse_paths: None,
        }))
    );
}

#[test]
fn rejects_three_segment_github_shorthand() {
    let r = parse_marketplace_input("owner/repo/extra", no_home);
    assert_eq!(r, Ok(None));
}

#[test]
fn parses_ssh_git_standard() {
    let r = parse_marketplace_input("git@github.com:anthropics/claude-plugins.git", no_home);
    assert_eq!(
        r,
        Ok(Some(MarketplaceSource::Git {
            url: "git@github.com:anthropics/claude-plugins.git".into(),
            git_ref: None,
            path: None,
            sparse_paths: None,
        }))
    );
}

#[test]
fn parses_ssh_git_with_ref() {
    let r = parse_marketplace_input("git@github.com:owner/repo.git#feature-branch", no_home);
    assert_eq!(
        r,
        Ok(Some(MarketplaceSource::Git {
            url: "git@github.com:owner/repo.git".into(),
            git_ref: Some("feature-branch".into()),
            path: None,
            sparse_paths: None,
        }))
    );
}

#[test]
fn parses_ssh_git_with_enterprise_username() {
    let r = parse_marketplace_input("org-123456@github.com:owner/repo.git", no_home);
    assert_eq!(
        r,
        Ok(Some(MarketplaceSource::Git {
            url: "org-123456@github.com:owner/repo.git".into(),
            git_ref: None,
            path: None,
            sparse_paths: None,
        }))
    );
}

#[test]
fn parses_ssh_git_custom_username_and_host() {
    let r = parse_marketplace_input("deploy@gitlab.com:group/project.git", no_home);
    assert_eq!(
        r,
        Ok(Some(MarketplaceSource::Git {
            url: "deploy@gitlab.com:group/project.git".into(),
            git_ref: None,
            path: None,
            sparse_paths: None,
        }))
    );
}

#[test]
fn parses_https_dotgit_as_git_source() {
    let r = parse_marketplace_input("https://gitlab.com/group/repo.git", no_home);
    assert_eq!(
        r,
        Ok(Some(MarketplaceSource::Git {
            url: "https://gitlab.com/group/repo.git".into(),
            git_ref: None,
            path: None,
            sparse_paths: None,
        }))
    );
}

#[test]
fn parses_https_dotgit_with_ref() {
    let r = parse_marketplace_input("https://example.com/repo.git#dev", no_home);
    assert_eq!(
        r,
        Ok(Some(MarketplaceSource::Git {
            url: "https://example.com/repo.git".into(),
            git_ref: Some("dev".into()),
            path: None,
            sparse_paths: None,
        }))
    );
}

#[test]
fn parses_azure_devops_git_path_as_git_source() {
    let r = parse_marketplace_input("https://dev.azure.com/org/proj/_git/repo", no_home);
    assert_eq!(
        r,
        Ok(Some(MarketplaceSource::Git {
            url: "https://dev.azure.com/org/proj/_git/repo".into(),
            git_ref: None,
            path: None,
            sparse_paths: None,
        }))
    );
}

#[test]
fn parses_https_github_appends_dotgit() {
    let r = parse_marketplace_input("https://github.com/anthropics/claude-plugins", no_home);
    // Keeps the HTTPS shape and routes through `git` (cloning) but appends `.git`.
    assert_eq!(
        r,
        Ok(Some(MarketplaceSource::Git {
            url: "https://github.com/anthropics/claude-plugins.git".into(),
            git_ref: None,
            path: None,
            sparse_paths: None,
        }))
    );
}

#[test]
fn parses_https_github_keeps_existing_dotgit() {
    let r = parse_marketplace_input("https://github.com/anthropics/claude-plugins.git", no_home);
    assert_eq!(
        r,
        Ok(Some(MarketplaceSource::Git {
            url: "https://github.com/anthropics/claude-plugins.git".into(),
            git_ref: None,
            path: None,
            sparse_paths: None,
        }))
    );
}

#[test]
fn parses_generic_https_url_as_url_source() {
    let r = parse_marketplace_input("https://example.com/marketplace.json", no_home);
    assert_eq!(
        r,
        Ok(Some(MarketplaceSource::Url {
            url: "https://example.com/marketplace.json".into(),
            headers: None,
        }))
    );
}

#[test]
fn parses_local_existing_directory() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir_str = tmp.path().to_string_lossy().to_string();
    let r = parse_marketplace_input(&dir_str, no_home);
    assert_eq!(r, Ok(Some(MarketplaceSource::Directory { path: dir_str })));
}

#[test]
fn parses_local_existing_json_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("marketplace.json");
    std::fs::write(&path, b"{}").expect("write file");
    let path_str = path.to_string_lossy().to_string();
    let r = parse_marketplace_input(&path_str, no_home);
    assert_eq!(r, Ok(Some(MarketplaceSource::File { path: path_str })));
}

#[test]
fn rejects_local_non_json_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("marketplace.txt");
    std::fs::write(&path, b"not json").expect("write file");
    let path_str = path.to_string_lossy().to_string();
    let r = parse_marketplace_input(&path_str, no_home);
    match r {
        Err(ParseError::NonJsonFile { path }) => assert!(path.contains("marketplace.txt")),
        other => panic!("expected NonJsonFile, got {other:?}"),
    }
}

#[test]
fn handles_missing_local_path() {
    let r = parse_marketplace_input("/definitely/does/not/exist/marketplace", no_home);
    match r {
        Err(ParseError::PathDoesNotExist { path }) => assert!(path.contains("does/not/exist")),
        other => panic!("expected PathDoesNotExist, got {other:?}"),
    }
}

#[test]
fn tilde_without_home_returns_error() {
    let r = parse_marketplace_input("~", no_home);
    match r {
        Err(ParseError::HomeUnresolvable { input }) => assert_eq!(input, "~"),
        other => panic!("expected HomeUnresolvable, got {other:?}"),
    }
}

#[test]
fn tilde_expansion_uses_provided_home() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let r = parse_marketplace_input("~", || Some(tmp.path().to_path_buf()));
    assert!(
        matches!(r, Ok(Some(MarketplaceSource::Directory { .. }))),
        "expected Directory, got {r:?}"
    );
}

#[test]
fn unrecognised_input_returns_ok_none() {
    assert_eq!(parse_marketplace_input("just-a-word", no_home), Ok(None));
    assert_eq!(parse_marketplace_input("@scoped/name", no_home), Ok(None));
}

#[test]
fn empty_input_returns_ok_none() {
    assert_eq!(parse_marketplace_input("", no_home), Ok(None));
    assert_eq!(parse_marketplace_input("   ", no_home), Ok(None));
}

#[test]
fn derive_name_handles_each_source() {
    assert_eq!(
        derive_marketplace_name(&MarketplaceSource::Github {
            repo: "anthropics/claude-plugins".into(),
            git_ref: None,
            path: None,
            sparse_paths: None,
        }),
        "claude-plugins"
    );
    assert_eq!(
        derive_marketplace_name(&MarketplaceSource::Git {
            url: "git@github.com:owner/cool-repo.git".into(),
            git_ref: None,
            path: None,
            sparse_paths: None,
        }),
        "cool-repo"
    );
    assert_eq!(
        derive_marketplace_name(&MarketplaceSource::Url {
            url: "https://example.com/some/path/marketplace.json".into(),
            headers: None,
        }),
        "marketplace"
    );
    assert_eq!(
        derive_marketplace_name(&MarketplaceSource::Directory {
            path: "/tmp/my-marketplace".into(),
        }),
        "my-marketplace"
    );
    assert_eq!(
        derive_marketplace_name(&MarketplaceSource::Npm {
            package: "@scope/marketplace".into(),
        }),
        "@scope/marketplace"
    );
}
