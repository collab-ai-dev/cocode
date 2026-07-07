use std::path::Path;
use std::path::PathBuf;

use pretty_assertions::assert_eq;

use super::RuntimePaths;

#[test]
fn default_memory_base_is_config_home() {
    let paths = RuntimePaths::new(PathBuf::from("/home/u/.coco"), None);

    assert_eq!(paths.config_home(), Path::new("/home/u/.coco"));
    assert_eq!(paths.memory_base(), Path::new("/home/u/.coco"));
    assert_eq!(
        paths.projects_root(),
        PathBuf::from("/home/u/.coco/projects")
    );
    assert_eq!(
        paths.user_memory_dir(),
        PathBuf::from("/home/u/.coco/memory")
    );
}

#[test]
fn remote_memory_override_only_changes_memory_base() {
    let paths = RuntimePaths::new(
        PathBuf::from("/home/u/.coco"),
        Some(PathBuf::from("/remote/memory")),
    );

    assert_eq!(paths.config_home(), Path::new("/home/u/.coco"));
    assert_eq!(paths.memory_base(), Path::new("/remote/memory"));
    assert_eq!(
        paths.projects_root(),
        PathBuf::from("/remote/memory/projects")
    );
    assert_eq!(
        paths.user_memory_dir(),
        PathBuf::from("/remote/memory/memory")
    );
    assert_eq!(
        paths.sessions_dir(),
        PathBuf::from("/home/u/.coco/sessions")
    );
    assert_eq!(
        paths.pids_dir(),
        PathBuf::from("/home/u/.coco/sessions/pids")
    );
}

#[test]
fn runtime_config_home_subpaths_are_stable() {
    let paths = RuntimePaths::new(PathBuf::from("/cfg"), Some(PathBuf::from("/mem")));

    assert_eq!(paths.logs_dir(), PathBuf::from("/cfg/logs"));
    assert_eq!(paths.plugins_dir(), PathBuf::from("/cfg/plugins"));
    assert_eq!(
        paths.output_styles_dir(),
        PathBuf::from("/cfg/output-styles")
    );
    assert_eq!(paths.models_file(), PathBuf::from("/cfg/models.json"));
    assert_eq!(paths.file_history_dir(), PathBuf::from("/cfg/file-history"));
}

#[test]
fn project_paths_derive_from_memory_base() {
    let paths = RuntimePaths::new(PathBuf::from("/cfg"), Some(PathBuf::from("/mem")));
    let project = paths.project_paths(Path::new("/repo"));

    assert_eq!(project.memory_base(), Path::new("/mem"));
    assert_eq!(project.project_dir(), PathBuf::from("/mem/projects/-repo"));
}
