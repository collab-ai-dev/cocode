use std::path::Path;

use pretty_assertions::assert_eq;

use super::*;

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, content).expect("write");
}

#[test]
fn memory_discovery_uses_runtime_memory_base_override() {
    let config_home = tempfile::tempdir().expect("config home");
    let remote = tempfile::tempdir().expect("remote memory");
    let cwd = tempfile::tempdir().expect("project");
    let paths = coco_paths::RuntimePaths::new(
        config_home.path().to_path_buf(),
        Some(remote.path().to_path_buf()),
    );
    let project = paths.project_paths(cwd.path());

    write(&config_home.path().join("memory/ignored.md"), "ignored");
    write(&paths.user_memory_dir().join("user.md"), "user");
    write(&project.memory_dir().join("auto.md"), "auto");
    write(&project.team_memory_dir().join("team.md"), "team");

    let mut found: Vec<_> = get_memory_files_with_runtime_paths(cwd.path(), &paths)
        .into_iter()
        .map(|file| (file.memory_type, file.content))
        .collect();
    found.sort_by_key(|(_, content)| content.clone());

    assert_eq!(
        found,
        vec![
            (MemoryType::AutoMem, "auto".to_string()),
            (MemoryType::TeamMem, "team".to_string()),
            (MemoryType::User, "user".to_string()),
        ]
    );
}

#[test]
fn memory_path_classification_uses_runtime_memory_base_override() {
    let config_home = tempfile::tempdir().expect("config home");
    let remote = tempfile::tempdir().expect("remote memory");
    let cwd = tempfile::tempdir().expect("project");
    let paths = coco_paths::RuntimePaths::new(
        config_home.path().to_path_buf(),
        Some(remote.path().to_path_buf()),
    );
    let project = paths.project_paths(cwd.path());
    let auto_path = project.memory_dir().join("MEMORY.md");
    let team_path = project.team_memory_dir().join("MEMORY.md");
    let config_home_auto = coco_paths::RuntimePaths::new(config_home.path().to_path_buf(), None)
        .project_paths(cwd.path())
        .memory_dir()
        .join("MEMORY.md");

    assert!(is_memory_managed_path_with_runtime_paths(
        &auto_path,
        cwd.path(),
        &paths
    ));
    assert_eq!(
        classify_memory_path_with_runtime_paths(&team_path, cwd.path(), &paths),
        Some(MemoryType::TeamMem)
    );
    assert!(!is_memory_managed_path_with_runtime_paths(
        &config_home_auto,
        cwd.path(),
        &paths
    ));
}
