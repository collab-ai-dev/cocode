use super::default_watch_paths;
use coco_skills::watcher::SkillReloadScope;
use coco_skills::watcher::session_reload_scopes;
use std::path::Path;

#[test]
fn default_watch_paths_covers_user_and_project_scopes() {
    let cwd = Path::new("/proj");
    let config_home = Path::new("/home/.coco");
    let paths = default_watch_paths(cwd, config_home);
    assert_eq!(paths[0], Path::new("/home/.coco/skills").to_path_buf());
    assert!(paths.contains(&Path::new("/proj/.coco/skills").to_path_buf()));
    assert!(paths.contains(&Path::new("/.coco/skills").to_path_buf()));
    assert!(
        !paths
            .iter()
            .any(|path| path.to_string_lossy().contains(".claude"))
    );
}

#[test]
fn reload_scopes_include_managed_but_watch_paths_do_not() {
    let cwd = Path::new("/proj");
    let config_home = Path::new("/home/.coco");
    let scopes = session_reload_scopes(config_home, cwd);
    assert!(
        scopes
            .iter()
            .any(|scope| matches!(scope, SkillReloadScope::Managed(_)))
    );

    let watch_paths = default_watch_paths(cwd, config_home);
    let managed = coco_skills::get_managed_skills_path();
    assert!(!watch_paths.contains(&managed));
}
