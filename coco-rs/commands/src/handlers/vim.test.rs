use super::*;

#[tokio::test]
async fn toggle_flips_mode_and_persists() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().to_path_buf();

    // First call defaults from "normal" → toggles to "vim".
    let out1 = handler_with_home(home.clone(), String::new())
        .await
        .unwrap();
    assert!(out1.contains("vim"));
    let path = home.join(".coco").join("state").join("editor_mode");
    assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), "vim");

    // Second toggle goes back to normal.
    let out2 = handler_with_home(home.clone(), "toggle".to_string())
        .await
        .unwrap();
    assert!(out2.contains("normal"));
    assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), "normal");
}

#[tokio::test]
async fn explicit_arg_sets_mode() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().to_path_buf();

    handler_with_home(home.clone(), "vim".to_string())
        .await
        .unwrap();
    let path = home.join(".coco").join("state").join("editor_mode");
    assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), "vim");

    handler_with_home(home.clone(), "normal".to_string())
        .await
        .unwrap();
    assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), "normal");
}

#[tokio::test]
async fn unknown_arg_returns_error_text() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = handler_with_home(tmp.path().to_path_buf(), "emacsy".to_string())
        .await
        .unwrap();
    assert!(out.contains("Unknown editor mode"));
}
