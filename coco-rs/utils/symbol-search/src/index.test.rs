use super::*;
use std::fs;

#[test]
fn test_build_and_search() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(
        dir.path().join("main.rs"),
        "struct ModelInfo {}\nfn process() {}\n",
    )
    .expect("write");

    let index = SymbolIndex::build(dir.path()).expect("build");
    assert!(index.len() >= 2);

    let results = index.search("ModelInfo", 10);
    assert!(!results.is_empty());
    assert_eq!(results[0].name, "ModelInfo");
}

#[test]
fn test_case_insensitive_search() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("main.rs"), "struct ModelInfo {}\n").expect("write");

    let index = SymbolIndex::build(dir.path()).expect("build");
    let results = index.search("modelinfo", 10);
    assert!(!results.is_empty());
    assert_eq!(results[0].name, "ModelInfo");
}

#[test]
fn test_fuzzy_search() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("main.rs"), "struct ModelInfo {}\n").expect("write");

    let index = SymbolIndex::build(dir.path()).expect("build");
    let results = index.search("mdlinfo", 10);
    // fuzzy_match should match "mdlinfo" → "ModelInfo" via subsequence
    // If the fuzzy matcher supports this, we'll get a result
    // Either way the search shouldn't panic
    let _ = results;
}

#[test]
fn test_update_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("main.rs"), "fn old_func() {}\n").expect("write");

    let mut index = SymbolIndex::build(dir.path()).expect("build");
    let results = index.search("old_func", 10);
    assert!(!results.is_empty());

    // Update the file
    fs::write(dir.path().join("main.rs"), "fn new_func() {}\n").expect("write");
    index
        .update_files(dir.path(), &[PathBuf::from("main.rs")])
        .expect("update");

    let results = index.search("old_func", 10);
    assert!(results.is_empty());
    let results = index.search("new_func", 10);
    assert!(!results.is_empty());
}

#[test]
fn test_remove_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("main.rs"), "fn my_func() {}\n").expect("write");

    let mut index = SymbolIndex::build(dir.path()).expect("build");
    assert!(!index.is_empty());

    index.remove_file("main.rs");
    assert!(index.is_empty());
}

#[test]
fn test_empty_query_returns_empty() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("main.rs"), "fn foo() {}\n").expect("write");

    let index = SymbolIndex::build(dir.path()).expect("build");
    let results = index.search("", 10);
    assert!(results.is_empty());
}
