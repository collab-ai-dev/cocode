use super::*;

#[tokio::test]
async fn creates_template_when_missing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().to_path_buf();

    let out = handler_with_overrides(home.clone(), String::new())
        .await
        .unwrap();
    let path = home.join(".coco").join("keybindings.json");
    assert!(path.exists(), "expected file at {}", path.display());
    let body = tokio::fs::read_to_string(&path).await.unwrap();
    assert!(body.contains("bindings"));
    assert!(out.contains("Created"));
    assert!(out.contains("EDITOR"));
}

#[tokio::test]
async fn preserves_existing_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().to_path_buf();

    let path = home.join(".coco").join("keybindings.json");
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&path, "{\"bindings\":[{\"chord\":\"ctrl+z\"}]}\n")
        .await
        .unwrap();

    let out = handler_with_overrides(home, String::new()).await.unwrap();
    let body = tokio::fs::read_to_string(&path).await.unwrap();
    assert!(body.contains("ctrl+z"), "user content was clobbered");
    assert!(out.contains("Found"));
}
