use super::*;
use pretty_assertions::assert_eq;
use std::path::Path;

#[test]
fn personal_path_classifies_as_personal() {
    assert_eq!(
        memory_scope_for_path(Path::new("/m/user_role.md"), Path::new("/m")),
        MemoryScope::Personal
    );
}

#[test]
fn team_path_classifies_as_team() {
    assert_eq!(
        memory_scope_for_path(Path::new("/m/team/conventions.md"), Path::new("/m")),
        MemoryScope::Team
    );
}

#[test]
fn dangerous_dir_bypass_disabled_with_override() {
    assert!(!should_bypass_dangerous_dirs(
        Path::new("/m/x.md"),
        Path::new("/m"),
        true,
    ));
}

#[test]
fn dangerous_dir_bypass_enabled_for_memdir_paths() {
    assert!(should_bypass_dangerous_dirs(
        Path::new("/m/x.md"),
        Path::new("/m"),
        false,
    ));
}

#[test]
fn auto_mem_file_requires_markdown_under_memory_dir() {
    assert!(is_auto_mem_file(Path::new("/m/x.md"), Path::new("/m")));
    assert!(!is_auto_mem_file(Path::new("/m/x.txt"), Path::new("/m")));
    assert!(!is_auto_mem_file(
        Path::new("/outside/x.md"),
        Path::new("/m")
    ));
}

#[test]
fn dangerous_dir_bypass_ignores_non_markdown_memdir_paths() {
    assert!(!should_bypass_dangerous_dirs(
        Path::new("/m/x.txt"),
        Path::new("/m"),
        false,
    ));
}

#[test]
fn classify_written_path_requires_markdown_for_auto_and_team_memory() {
    assert_eq!(
        classify_written_path(Path::new("/m/team/conventions.md"), Path::new("/m"), None),
        WriteClassification::TeamMem
    );
    assert_eq!(
        classify_written_path(Path::new("/m/team/conventions.txt"), Path::new("/m"), None),
        WriteClassification::Unrelated
    );
    assert_eq!(
        classify_written_path(Path::new("/m/user_role.md"), Path::new("/m"), None),
        WriteClassification::AutoMem
    );
    assert_eq!(
        classify_written_path(Path::new("/m/user_role.txt"), Path::new("/m"), None),
        WriteClassification::Unrelated
    );
}
