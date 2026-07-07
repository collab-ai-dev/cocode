use super::*;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

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
