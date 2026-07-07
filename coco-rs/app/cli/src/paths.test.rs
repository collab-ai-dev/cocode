use std::path::Path;
use std::sync::Mutex;

use super::*;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn session_workspace_uses_cwd_as_project_root_outside_git() {
    let temp = tempdir().unwrap();
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&cwd).unwrap();

    let workspace = SessionWorkspace::resolve(cwd.clone());

    assert_eq!(workspace.cwd, cwd);
    assert_eq!(workspace.project_root, workspace.cwd);
    assert_eq!(
        workspace.storage_paths.project_dir(),
        project_paths(&workspace.cwd).project_dir()
    );
}

#[test]
fn session_workspace_uses_git_root_as_project_root() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    let nested = repo.join("app").join("crate");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(&nested).unwrap();

    let workspace = SessionWorkspace::resolve(nested.clone());

    assert_eq!(workspace.cwd, nested);
    assert_eq!(workspace.project_root, repo);
    assert_eq!(
        workspace.storage_paths.project_dir(),
        project_paths(&workspace.cwd).project_dir()
    );
}

#[test]
fn runtime_paths_uses_remote_memory_dir_for_project_scoped_paths() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let config_home =
        std::env::temp_dir().join(format!("coco-paths-cfg-{}", uuid::Uuid::new_v4().simple()));
    let memory_home =
        std::env::temp_dir().join(format!("coco-paths-mem-{}", uuid::Uuid::new_v4().simple()));
    let previous_config = std::env::var_os(coco_utils_common::COCO_CONFIG_DIR_ENV);
    let previous_memory = std::env::var_os(EnvKey::CocoRemoteMemoryDir);

    unsafe {
        std::env::set_var(coco_utils_common::COCO_CONFIG_DIR_ENV, &config_home);
        std::env::set_var(EnvKey::CocoRemoteMemoryDir.as_str(), &memory_home);
    }

    let paths = runtime_paths();
    let project = paths.project_paths(Path::new("/repo"));

    assert_eq!(paths.config_home(), config_home.as_path());
    assert_eq!(paths.memory_base(), memory_home.as_path());
    assert!(
        project
            .project_dir()
            .starts_with(memory_home.join("projects"))
    );
    assert_eq!(paths.logs_dir(), config_home.join("logs"));
    assert_eq!(paths.plugins_dir(), config_home.join("plugins"));
    assert_eq!(paths.file_history_dir(), config_home.join("file-history"));

    restore_env(coco_utils_common::COCO_CONFIG_DIR_ENV, previous_config);
    restore_env(EnvKey::CocoRemoteMemoryDir.as_str(), previous_memory);
}

#[test]
fn project_output_style_dirs_walk_from_cwd_to_git_root() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    let nested = repo.join("app").join("crate");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(
        nested
            .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
            .join("output-styles"),
    )
    .unwrap();
    std::fs::create_dir_all(
        repo.join(coco_utils_common::COCO_CONFIG_DIR_NAME)
            .join("output-styles"),
    )
    .unwrap();
    std::fs::create_dir_all(
        temp.path()
            .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
            .join("output-styles"),
    )
    .unwrap();

    let dirs = project_output_style_dirs(&nested);

    assert_eq!(
        dirs,
        vec![
            nested
                .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
                .join("output-styles"),
            repo.join(coco_utils_common::COCO_CONFIG_DIR_NAME)
                .join("output-styles"),
        ]
    );
}

#[test]
fn project_output_style_dirs_only_returns_existing_dirs() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    let nested = repo.join("src");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::create_dir_all(
        repo.join(coco_utils_common::COCO_CONFIG_DIR_NAME)
            .join("output-styles"),
    )
    .unwrap();

    let dirs = project_output_style_dirs(&nested);

    assert_eq!(
        dirs,
        vec![
            repo.join(coco_utils_common::COCO_CONFIG_DIR_NAME)
                .join("output-styles")
        ]
    );
}

fn restore_env(key: &str, value: Option<std::ffi::OsString>) {
    unsafe {
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }
}
