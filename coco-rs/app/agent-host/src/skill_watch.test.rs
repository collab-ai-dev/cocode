use super::default_watch_paths;
use coco_skills::watcher::SkillReloadScope;
use coco_skills::watcher::session_reload_scopes;
use std::path::Path;
use std::path::PathBuf;

fn config_home(root: &str) -> PathBuf {
    PathBuf::from(format!(
        "{root}/{}",
        coco_utils_common::COCO_CONFIG_DIR_NAME
    ))
}

#[test]
fn default_watch_paths_covers_user_and_project_scopes() {
    let cwd = Path::new("/proj");
    let home = config_home("/home");
    let paths = default_watch_paths(cwd, &home);
    assert_eq!(paths[0], home.join("skills"));
    assert!(paths.contains(&config_home("/proj").join("skills")));
    assert!(paths.contains(&config_home("").join("skills")));
    assert!(
        !paths
            .iter()
            .any(|path| path.to_string_lossy().contains(".claude"))
    );
}

#[test]
fn reload_scopes_include_managed_but_watch_paths_do_not() {
    let cwd = Path::new("/proj");
    let home = config_home("/home");
    let scopes = session_reload_scopes(&home, cwd);
    assert!(
        scopes
            .iter()
            .any(|scope| matches!(scope, SkillReloadScope::Managed(_)))
    );

    let watch_paths = default_watch_paths(cwd, &home);
    let managed = coco_skills::get_managed_skills_path();
    assert!(!watch_paths.contains(&managed));
}
