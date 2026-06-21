use super::*;

#[test]
fn test_readme_important() {
    assert!(is_important("README.md"));
    assert!(is_important("README"));
    assert!(is_important("README.txt"));
    assert!(is_important("src/README.md")); // In subdirectory
}

#[test]
fn test_cargo_toml_important() {
    assert!(is_important("Cargo.toml"));
    assert!(is_important("Cargo.lock"));
}

#[test]
fn test_package_json_important() {
    assert!(is_important("package.json"));
    assert!(is_important("package-lock.json"));
    assert!(is_important("yarn.lock"));
}

#[test]
fn test_docker_important() {
    assert!(is_important("Dockerfile"));
    assert!(is_important("docker-compose.yml"));
}

#[test]
fn test_github_workflows() {
    assert!(is_important(".github/workflows/ci.yml"));
    assert!(is_important(".github/workflows/test.yaml"));
    assert!(is_important(".github/workflows/build.yml"));
}

#[test]
fn test_wildcard_extension() {
    assert!(is_important("project.csproj"));
    assert!(is_important("solution.sln"));
}

#[test]
fn test_path_patterns() {
    assert!(is_important(".cargo/config.toml"));
    assert!(is_important(".circleci/config.yml"));
}

#[test]
fn test_not_important() {
    assert!(!is_important("main.rs"));
    assert!(!is_important("src/lib.rs"));
    assert!(!is_important("utils.py"));
    assert!(!is_important("random.txt"));
}

#[test]
fn test_filter_important_files() {
    let files = vec![
        "README.md".to_string(),
        "src/main.rs".to_string(),
        "Cargo.toml".to_string(),
        "src/lib.rs".to_string(),
        ".github/workflows/ci.yml".to_string(),
    ];

    let important = filter_important_files(&files);

    assert_eq!(important.len(), 3);
    assert!(important.contains(&"README.md".to_string()));
    assert!(important.contains(&"Cargo.toml".to_string()));
    assert!(important.contains(&".github/workflows/ci.yml".to_string()));
}
