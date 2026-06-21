use super::*;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

#[test]
fn project_output_style_dirs_walk_from_cwd_to_git_root() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    let nested = repo.join("app").join("crate");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(nested.join(".coco").join("output-styles")).unwrap();
    std::fs::create_dir_all(repo.join(".coco").join("output-styles")).unwrap();
    std::fs::create_dir_all(temp.path().join(".coco").join("output-styles")).unwrap();

    let dirs = project_output_style_dirs(&nested);

    assert_eq!(
        dirs,
        vec![
            nested.join(".coco").join("output-styles"),
            repo.join(".coco").join("output-styles"),
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
    std::fs::create_dir_all(repo.join(".coco").join("output-styles")).unwrap();

    let dirs = project_output_style_dirs(&nested);

    assert_eq!(dirs, vec![repo.join(".coco").join("output-styles")]);
}
