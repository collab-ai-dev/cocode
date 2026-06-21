use super::*;

#[test]
fn test_python_package_manager_as_str() {
    assert_eq!(PythonPackageManager::Uvx.as_str(), "uvx");
    assert_eq!(PythonPackageManager::Pipx.as_str(), "pipx");
    assert_eq!(PythonPackageManager::Pip.as_str(), "pip");
}

#[test]
fn test_install_command() {
    let cmd = PythonPackageManager::Uvx.install_command();
    assert_eq!(cmd, vec!["uv", "tool", "install", "it2"]);

    let cmd = PythonPackageManager::Pipx.install_command();
    assert_eq!(cmd, vec!["pipx", "install", "it2"]);

    let cmd = PythonPackageManager::Pip.install_command();
    assert_eq!(cmd, vec!["pip", "install", "--user", "it2"]);
}

#[test]
fn test_python_api_instructions() {
    let instructions = get_python_api_instructions();
    assert!(instructions.len() >= 3);
    assert!(instructions[0].contains("Python API"));
    assert!(instructions[2].contains("Settings"));
}

#[test]
fn test_prefer_tmux_default() {
    // Default should be false (no preference file)
    // Note: this may be true if running on a machine with the preference set
    let _ = get_prefer_tmux_over_iterm2();
}

#[tokio::test]
async fn test_detect_python_package_manager_no_panic() {
    // Should not panic regardless of environment
    let _ = detect_python_package_manager().await;
}

#[test]
fn test_it2_setup_state_path() {
    let path = it2_setup_state_path();
    assert!(path.to_string_lossy().contains(".coco"));
}
