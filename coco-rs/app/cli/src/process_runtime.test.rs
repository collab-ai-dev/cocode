use std::sync::Arc;
use std::time::Duration;

use tempfile::tempdir;

use super::*;

#[tokio::test]
async fn process_runtime_global_reuses_single_owner() {
    let first = ProcessRuntime::global();
    let second = ProcessRuntime::global();

    assert!(Arc::ptr_eq(&first, &second));
}

#[tokio::test]
async fn process_runtime_reuses_project_services_through_owned_registry() {
    let temp = tempdir().unwrap();
    let config_home = temp.path().join("home");
    let project_root = temp.path().join("repo");
    std::fs::create_dir_all(&config_home).unwrap();
    std::fs::create_dir_all(&project_root).unwrap();
    let registry = Box::leak(Box::new(ProjectRegistry::new()));
    let runtime = ProcessRuntime::start(registry, Duration::ZERO, Duration::from_secs(60));

    let first = runtime.project_services(&config_home, project_root.clone());
    let second = runtime.project_services(&config_home, project_root);

    assert!(Arc::ptr_eq(&first, &second));
}

#[tokio::test]
async fn process_runtime_reload_replaces_project_services() {
    let temp = tempdir().unwrap();
    let config_home = temp.path().join("home");
    let project_root = temp.path().join("repo");
    std::fs::create_dir_all(&config_home).unwrap();
    std::fs::create_dir_all(&project_root).unwrap();
    let registry = Box::leak(Box::new(ProjectRegistry::new()));
    let runtime = ProcessRuntime::start(registry, Duration::ZERO, Duration::from_secs(60));

    let first = runtime.project_services(&config_home, project_root.clone());
    let second = runtime.reload_project_services(&config_home, project_root);

    assert!(!Arc::ptr_eq(&first, &second));
}
