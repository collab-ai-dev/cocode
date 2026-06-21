//! Tests for paste handling.

use crate::paste::PasteManager;
use crate::paste::is_paste_pill;

#[test]
fn test_is_paste_pill() {
    assert!(is_paste_pill("[Pasted text #1]"));
    assert!(is_paste_pill("[Pasted text #42]"));
    assert!(is_paste_pill("[Image #1]"));
    assert!(!is_paste_pill("[Something else]"));
    assert!(!is_paste_pill("not a pill"));
    assert!(!is_paste_pill("[unclosed"));
}

#[test]
fn test_paste_manager_text() {
    let mut mgr = PasteManager::new();
    let pill = mgr.add_text("hello world".to_string());
    assert_eq!(pill, "[Pasted text #1]");
    assert_eq!(mgr.entries().len(), 1);
    assert!(!mgr.entries()[0].is_image);
}

#[test]
fn test_paste_manager_resolve() {
    let mut mgr = PasteManager::new();
    mgr.add_text("content A".to_string());
    mgr.add_text("content B".to_string());

    let input = "See [Pasted text #1] and [Pasted text #2]";
    let resolved = mgr.resolve(input);
    assert_eq!(resolved, "See content A and content B");
}

#[test]
fn test_paste_manager_numbering() {
    let mut mgr = PasteManager::new();
    let p1 = mgr.add_text("a".to_string());
    let p2 = mgr.add_image_data(vec![0x89, 0x50], "image/png".to_string());
    assert_eq!(p1, "[Pasted text #1]");
    assert_eq!(p2, "[Image #2]");
}

#[test]
fn test_resolve_structured_image_keeps_pill_and_carries_bytes() {
    let mut mgr = PasteManager::new();
    let pill = mgr.add_image_data(vec![1, 2, 3], "image/png".to_string());
    let resolved = mgr.resolve_structured(&format!("{pill} what is this?"));
    // The `[Image #N]` placeholder survives inline (mirrors TS); bytes ship
    // separately for the image content block.
    assert_eq!(resolved.text, "[Image #1] what is this?");
    assert_eq!(resolved.images.len(), 1);
    assert_eq!(resolved.images[0].bytes, vec![1, 2, 3]);
    assert_eq!(resolved.images[0].mime, "image/png");
}

#[test]
fn test_resolve_structured_image_only_keeps_pill() {
    let mut mgr = PasteManager::new();
    let pill = mgr.add_image_data(vec![9], "image/jpeg".to_string());
    let resolved = mgr.resolve_structured(&pill);
    // An image-only submit is still non-empty, so it clears the submit guard.
    assert_eq!(resolved.text, "[Image #1]");
    assert_eq!(resolved.images.len(), 1);
}

#[test]
fn test_resolve_structured_mixes_text_and_image_pills() {
    let mut mgr = PasteManager::new();
    let text_pill = mgr.add_text("BIG BLOCK".to_string());
    let image_pill = mgr.add_image_data(vec![7], "image/png".to_string());
    let resolved = mgr.resolve_structured(&format!("see {text_pill} and {image_pill}"));
    // Text pill expands; image pill stays inline.
    assert_eq!(resolved.text, "see BIG BLOCK and [Image #2]");
    assert_eq!(resolved.images.len(), 1);
}
