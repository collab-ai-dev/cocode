use super::*;
use std::path::Path;

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

fn init_repo(root: &Path, files: &[(&str, &str)]) {
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

#[test]
fn redact_credentials_strips_userinfo() {
    assert_eq!(
        redact_credentials("https://user:tok@github.com/o/r.git"),
        "https://***@github.com/o/r.git"
    );
    assert_eq!(redact_credentials("no creds here"), "no creds here");
}

#[test]
fn parse_origin_slug_handles_https_and_ssh() {
    assert_eq!(
        parse_origin_slug("https://github.com/owner/repo.git"),
        Some("owner/repo".to_string())
    );
    assert_eq!(
        parse_origin_slug("https://github.com/owner/repo"),
        Some("owner/repo".to_string())
    );
    assert_eq!(
        parse_origin_slug("git@github.com:owner/repo.git"),
        Some("owner/repo".to_string())
    );
    // Trailing slash + credentialed HTTPS.
    assert_eq!(
        parse_origin_slug("https://user@github.com/owner/repo/"),
        Some("owner/repo".to_string())
    );
    // Nested path → last two segments win.
    assert_eq!(
        parse_origin_slug("https://gitlab.example.com/group/sub/repo.git"),
        Some("sub/repo".to_string())
    );
}

#[test]
fn parse_origin_slug_rejects_shapes_without_owner_repo() {
    assert_eq!(parse_origin_slug(""), None);
    assert_eq!(parse_origin_slug("https://github.com/onlyone"), None);
    assert_eq!(parse_origin_slug("not a url"), None);
}

#[tokio::test]
async fn github_origin_slug_reads_origin_remote() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    init_repo(&repo, &[("README.md", "hi")]);
    git(
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/widgets.git",
        ],
        &repo,
    );
    assert_eq!(
        github_origin_slug(&repo).await,
        Some("acme/widgets".to_string())
    );
    // A repo with no origin yields None.
    let bare = tmp.path().join("noremote");
    init_repo(&bare, &[("a.md", "x")]);
    assert_eq!(github_origin_slug(&bare).await, None);
}

#[tokio::test]
async fn shallow_clone_clones_local_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    init_repo(&src, &[("README.md", "hi")]);

    let dest = tmp.path().join("dest");
    shallow_clone(
        &src.to_string_lossy(),
        &dest,
        &CloneOptions {
            recurse_submodules: true,
            ..Default::default()
        },
    )
    .await
    .expect("clone ok");

    assert!(dest.join("README.md").is_file());
}

#[tokio::test]
async fn shallow_clone_sparse_only_materializes_listed_subdir() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    init_repo(&src, &[("keep/a.txt", "a"), ("drop/b.txt", "b")]);

    let dest = tmp.path().join("dest");
    shallow_clone(
        &src.to_string_lossy(),
        &dest,
        &CloneOptions {
            sparse_paths: vec!["keep".to_string()],
            ..Default::default()
        },
    )
    .await
    .expect("sparse clone ok");

    assert!(dest.join("keep/a.txt").is_file(), "listed subdir present");
    assert!(!dest.join("drop").exists(), "unlisted subdir excluded");
}

#[tokio::test]
async fn pull_updates_existing_clone() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    init_repo(&src, &[("v.txt", "1")]);

    let dest = tmp.path().join("dest");
    shallow_clone(&src.to_string_lossy(), &dest, &CloneOptions::default())
        .await
        .expect("clone ok");
    assert!(!dest.join("new.txt").exists());

    // New commit upstream, then pull.
    std::fs::write(src.join("new.txt"), "x").unwrap();
    git(&["add", "-A"], &src);
    git(&["commit", "-q", "-m", "add new"], &src);

    pull(&dest, None).await.expect("pull ok");
    assert!(dest.join("new.txt").is_file(), "pulled the new commit");
}

#[tokio::test]
async fn shallow_clone_bad_url_yields_git_command_error() {
    let tmp = tempfile::tempdir().unwrap();
    let err = shallow_clone(
        &tmp.path().join("nope").to_string_lossy(),
        &tmp.path().join("dest"),
        &CloneOptions::default(),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(err, GitToolingError::GitCommand { .. }),
        "expected GitCommand, got {err:?}"
    );
}
