use super::*;
use crate::schemas::MarketplaceSource;
use crate::schemas::RemotePluginSource;

// ── pure helpers ──

#[test]
fn github_https_url_builds_dot_git() {
    assert_eq!(
        github_https_url("anthropics/claude-plugins-official"),
        "https://github.com/anthropics/claude-plugins-official.git"
    );
}

#[test]
fn install_location_appends_subpath() {
    let base = PathBuf::from("/cache/m");
    assert_eq!(install_location_for(base.clone(), None), base);
    assert_eq!(install_location_for(base.clone(), Some("")), base);
    assert_eq!(
        install_location_for(base.clone(), Some("sub/dir")),
        base.join("sub/dir")
    );
}

#[test]
fn sanitize_replaces_path_unsafe_chars() {
    assert_eq!(sanitize("a/b c@d"), "a-b-c-d");
    assert_eq!(sanitize("ok-name_1"), "ok-name_1");
}

// ── marketplace source dispatch (no network) ──

#[tokio::test]
async fn fetch_marketplace_npm_is_unsupported() {
    let tmp = tempfile::tempdir().unwrap();
    let err = fetch_marketplace(
        &MarketplaceSource::Npm {
            package: "foo".into(),
        },
        "foo",
        tmp.path(),
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("npm marketplace sources are not supported")
    );
}

#[tokio::test]
async fn fetch_marketplace_local_returns_path_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("local-mkt");
    let loc = fetch_marketplace(
        &MarketplaceSource::Directory {
            path: dir.to_string_lossy().to_string(),
        },
        "local-mkt",
        tmp.path(),
    )
    .await
    .unwrap();
    assert_eq!(loc, dir);
}

// ── real local git clone (exercises run_git without network) ──

fn git(args: &[&str], cwd: &Path) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .status()
        .expect("git available");
    assert!(status.success(), "git {args:?} failed");
}

#[tokio::test]
async fn fetch_marketplace_git_clones_local_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src-repo");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("marketplace.json"), r#"{"name":"x","plugins":[]}"#).unwrap();
    git(&["init", "-q", "-b", "main"], &src);
    git(&["config", "user.email", "t@t.t"], &src);
    git(&["config", "user.name", "t"], &src);
    git(&["add", "-A"], &src);
    git(&["commit", "-q", "-m", "init"], &src);

    let cache = tmp.path().join("marketplaces");
    let loc = fetch_marketplace(
        &MarketplaceSource::Git {
            url: src.to_string_lossy().to_string(),
            git_ref: None,
            path: None,
            sparse_paths: None,
        },
        "my-mkt",
        &cache,
    )
    .await
    .expect("clone succeeds");

    assert_eq!(loc, cache.join("my-mkt"));
    assert!(loc.join("marketplace.json").is_file(), "manifest cloned");

    // Idempotent re-fetch (pull path) keeps the clone usable.
    let loc2 = fetch_marketplace(
        &MarketplaceSource::Git {
            url: src.to_string_lossy().to_string(),
            git_ref: None,
            path: None,
            sparse_paths: None,
        },
        "my-mkt",
        &cache,
    )
    .await
    .expect("re-fetch succeeds");
    assert!(loc2.join("marketplace.json").is_file());
}

// ── Phase 2: remote plugin source materialization (local git) ──

fn init_plugin_repo(root: &Path, files: &[(&str, &str)]) {
    std::fs::create_dir_all(root).unwrap();
    for (rel, content) in files {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }
    git(&["init", "-q", "-b", "main"], root);
    git(&["config", "user.email", "t@t.t"], root);
    git(&["config", "user.name", "t"], root);
    git(&["add", "-A"], root);
    git(&["commit", "-q", "-m", "init"], root);
}

#[tokio::test]
async fn fetch_plugin_source_git_url_materializes_into_dest() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("plugin-repo");
    init_plugin_repo(
        &repo,
        &[("PLUGIN.toml", "name = \"p\"\nversion = \"1.0.0\"\n")],
    );

    let dest = tmp.path().join("dest");
    fetch_plugin_source(
        &RemotePluginSource::Url {
            url: repo.to_string_lossy().to_string(),
            git_ref: None,
            sha: None,
        },
        &dest,
    )
    .await
    .expect("git url plugin install");

    assert!(dest.join("PLUGIN.toml").is_file(), "plugin materialized");
    assert!(
        !dest.join(".git").exists(),
        ".git stripped from plugin cache"
    );
}

#[tokio::test]
async fn fetch_plugin_source_git_subdir_extracts_only_subdir() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("monorepo");
    init_plugin_repo(
        &repo,
        &[
            ("foo/PLUGIN.toml", "name = \"foo\"\n"),
            ("bar/PLUGIN.toml", "name = \"bar\"\n"),
        ],
    );

    let dest = tmp.path().join("dest");
    fetch_plugin_source(
        &RemotePluginSource::GitSubdir {
            url: repo.to_string_lossy().to_string(),
            path: "foo".to_string(),
            git_ref: None,
            sha: None,
        },
        &dest,
    )
    .await
    .expect("git-subdir plugin install");

    // Only the requested subdir's contents land in dest.
    assert!(dest.join("PLUGIN.toml").is_file());
    assert!(!dest.join("bar").exists(), "sibling subdir excluded");
}

#[tokio::test]
async fn fetch_plugin_source_git_subdir_rejects_traversal() {
    let tmp = tempfile::tempdir().unwrap();
    let err = fetch_plugin_source(
        &RemotePluginSource::GitSubdir {
            url: "https://example.com/x.git".to_string(),
            path: "../escape".to_string(),
            git_ref: None,
            sha: None,
        },
        &tmp.path().join("dest"),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, PluginError::PathTraversal { .. }));
}

#[tokio::test]
async fn git_clone_bad_url_yields_clone_failed() {
    let tmp = tempfile::tempdir().unwrap();
    let err = fetch_marketplace(
        &MarketplaceSource::Git {
            url: tmp
                .path()
                .join("does-not-exist")
                .to_string_lossy()
                .to_string(),
            git_ref: None,
            path: None,
            sparse_paths: None,
        },
        "broken",
        &tmp.path().join("marketplaces"),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(err, PluginError::GitCloneFailed { .. }),
        "expected GitCloneFailed, got {err:?}"
    );
}
