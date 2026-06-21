use super::*;

#[test]
fn test_image_data_from_clipboard() {
    let img = ImageData {
        bytes: vec![0x89, 0x50, 0x4E, 0x47],
        mime: "image/png".to_string(),
    };
    assert_eq!(img.mime, "image/png");
    assert_eq!(img.bytes.len(), 4);
}

#[tokio::test]
async fn test_has_clipboard_support() {
    let _ = has_clipboard_image_support().await;
}
