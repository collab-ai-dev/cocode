use super::*;
use pretty_assertions::assert_eq;
use std::collections::HashMap;

fn options_from(value: serde_json::Value) -> XaiVideoProviderOptions {
    let map: HashMap<String, serde_json::Value> =
        serde_json::from_value(value).expect("options map");
    let mut po = ProviderOptions::default();
    po.0.insert("xai".into(), map);
    extract_xai_video_options(&Some(po)).expect("extract")
}

#[test]
fn absent_options_default() {
    let opts = extract_xai_video_options(&None).expect("extract");
    assert!(opts.mode.is_none());
    assert!(opts.video_url.is_none());
    assert!(opts.extra.is_empty());
    assert_eq!(resolve_video_mode(&opts).expect("resolve"), None);
}

#[test]
fn parses_typed_fields_and_extras() {
    let opts = options_from(serde_json::json!({
        "mode": "extend-video",
        "videoUrl": "https://example.com/v.mp4",
        "pollIntervalMs": 10,
        "pollTimeoutMs": 500,
        "resolution": "720p",
        "custom_field": {"nested": true},
    }));
    assert_eq!(opts.mode, Some(XaiVideoMode::ExtendVideo));
    assert_eq!(opts.video_url.as_deref(), Some("https://example.com/v.mp4"));
    assert_eq!(opts.poll_interval_ms, Some(10));
    assert_eq!(opts.poll_timeout_ms, Some(500));
    assert_eq!(opts.resolution, Some(XaiVideoResolution::P720));
    assert_eq!(
        opts.extra.get("custom_field"),
        Some(&serde_json::json!({"nested": true}))
    );
}

#[test]
fn explicit_mode_wins() {
    let opts = options_from(serde_json::json!({
        "mode": "reference-to-video",
        "referenceImageUrls": ["https://example.com/a.png"],
    }));
    assert_eq!(
        resolve_video_mode(&opts).expect("resolve"),
        Some(XaiVideoMode::ReferenceToVideo)
    );
}

#[test]
fn video_url_auto_detects_edit_mode() {
    let opts = options_from(serde_json::json!({
        "videoUrl": "https://example.com/v.mp4",
    }));
    assert_eq!(
        resolve_video_mode(&opts).expect("resolve"),
        Some(XaiVideoMode::EditVideo)
    );
}

#[test]
fn reference_urls_auto_detect_r2v_mode() {
    let opts = options_from(serde_json::json!({
        "referenceImageUrls": ["https://example.com/a.png", "https://example.com/b.png"],
    }));
    assert_eq!(
        resolve_video_mode(&opts).expect("resolve"),
        Some(XaiVideoMode::ReferenceToVideo)
    );
}

#[test]
fn empty_reference_urls_are_rejected() {
    let opts = options_from(serde_json::json!({ "referenceImageUrls": [] }));
    let err = resolve_video_mode(&opts).expect_err("must reject");
    assert!(err.to_string().contains("referenceImageUrls"));
}

#[test]
fn more_than_seven_reference_urls_are_rejected() {
    let urls: Vec<String> = (0..8)
        .map(|i| format!("https://example.com/{i}.png"))
        .collect();
    let opts = options_from(serde_json::json!({ "referenceImageUrls": urls }));
    assert!(resolve_video_mode(&opts).is_err());
}

#[test]
fn edit_mode_without_video_url_is_rejected() {
    let opts = options_from(serde_json::json!({ "mode": "edit-video" }));
    let err = resolve_video_mode(&opts).expect_err("must reject");
    assert!(err.to_string().contains("videoUrl"));
}

#[test]
fn empty_video_url_is_rejected() {
    let opts = options_from(serde_json::json!({ "videoUrl": "" }));
    assert!(resolve_video_mode(&opts).is_err());
}
