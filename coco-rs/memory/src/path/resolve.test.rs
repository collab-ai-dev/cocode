use super::*;
use coco_paths::ProjectPaths;
use pretty_assertions::assert_eq;
use std::path::Path;
use std::path::PathBuf;

fn config_home(root: &str) -> PathBuf {
    PathBuf::from(format!(
        "{root}/{}",
        coco_utils_common::COCO_CONFIG_DIR_NAME
    ))
}

fn config_home_ref(root: &str) -> PathBuf {
    config_home(root)
}

#[test]
fn override_path_wins() {
    let home = config_home_ref("/home/u");
    let dir = MemoryDir::resolve(
        home.as_path(),
        Path::new("/work/repo"),
        Some(Path::new("/custom/memory")),
    );
    assert_eq!(dir.personal, PathBuf::from("/custom/memory"));
    assert_eq!(dir.team, PathBuf::from("/custom/memory/team"));
}

#[test]
fn default_layout_outside_git_uses_project_path() {
    // tmp dir won't be a git repo so canonicalization falls through to
    // the literal project_root. The slug is whatever
    // `coco_paths::ProjectPaths` produces for that path.
    let temp = tempfile::tempdir().unwrap();
    let home = config_home("/home/u");
    let dir = MemoryDir::resolve(home.as_path(), temp.path(), None);
    let project_paths = ProjectPaths::new(home, temp.path());
    assert_eq!(dir.personal, project_paths.memory_dir());
    assert_eq!(dir.team, project_paths.team_memory_dir());
}

#[test]
fn default_layout_matches_observed_ts_slug_for_known_cwd() {
    // The literal directory observed on this dev machine at
    // `config home/projects/-Users-linyuzhi-codespace-myagent-codex/`.
    // Our slug for the same cwd MUST match — pre-fix, the local
    // `sanitize_project_path` stripped the leading `/` and produced
    // `Users-…` instead of `-Users-…`, silently disagreeing with
    // every other coco-rs instance pointed at the same repo.
    let dir = MemoryDir::resolve(
        config_home("/home/u").as_path(),
        Path::new("/Users/linyuzhi/codespace/myagent/codex"),
        None,
    );
    assert_eq!(
        dir.personal,
        config_home("/home/u").join("projects/-Users-linyuzhi-codespace-myagent-codex/memory"),
    );
}

#[test]
fn personal_and_team_index_paths() {
    let dir = MemoryDir {
        personal: PathBuf::from("/m"),
        team: PathBuf::from("/m/team"),
    };
    assert_eq!(dir.personal_index(), PathBuf::from("/m/MEMORY.md"));
    assert_eq!(dir.team_index(), PathBuf::from("/m/team/MEMORY.md"));
}
