use super::*;

#[test]
fn test_form_data_text() {
    let form = FormData::new()
        .text("name", "value")
        .text("another", "test")
        .build();

    // Form is built successfully
    let _ = form;
}

#[test]
fn test_form_data_bytes() {
    let form = FormData::new()
        .bytes("file", b"hello world".to_vec(), "test.txt")
        .build();

    let _ = form;
}

#[test]
fn test_form_data_json() {
    let form = FormData::new()
        .json("data", &serde_json::json!({"key": "value"}))
        .build();

    let _ = form;
}
