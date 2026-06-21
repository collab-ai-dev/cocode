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
