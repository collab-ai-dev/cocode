use super::*;
use pretty_assertions::assert_eq;
use std::collections::HashMap;
use vercel_ai_provider::ProviderOptions;
use vercel_ai_provider::video_model::v4::VideoDuration;
use vercel_ai_provider::video_model::v4::VideoSize;

fn model() -> XaiVideoModel {
    let config = Arc::new(XaiConfig {
        provider: "xai.video".into(),
        base_url: "https://api.x.ai/v1".into(),
        headers: Arc::new(HashMap::new),
        client: None,
    });
    XaiVideoModel::new("grok-imagine-video", config)
}

fn xai_video_options(value: serde_json::Value) -> XaiVideoProviderOptions {
    let map: HashMap<String, serde_json::Value> =
        serde_json::from_value(value).expect("options map");
    let mut po = ProviderOptions::default();
    po.0.insert("xai".into(), map);
    extract_xai_video_options(&Some(po)).expect("extract")
}

fn plan(options: &VideoModelV4CallOptions, mut xai: XaiVideoProviderOptions) -> VideoRequestPlan {
    let extras = std::mem::take(&mut xai.extra);
    plan_video_request("grok-imagine-video", options, &xai, extras).expect("plan")
}

#[test]
fn model_exposes_provider_and_id() {
    let m = model();
    assert_eq!(m.provider(), "xai.video");
    assert_eq!(m.model_id(), "grok-imagine-video");
}

#[test]
fn plan_default_generation_body() {
    let options = VideoModelV4CallOptions::new("a cat playing piano");
    let p = plan(&options, XaiVideoProviderOptions::default());
    assert_eq!(p.endpoint, "/videos/generations");
    assert_eq!(
        p.body,
        serde_json::json!({
            "model": "grok-imagine-video",
            "prompt": "a cat playing piano",
        })
    );
}

#[test]
fn plan_maps_duration_and_sdk_resolution() {
    let options = VideoModelV4CallOptions::new("p")
        .with_duration(VideoDuration::Seconds10)
        .with_size(VideoSize::HD720p);
    let p = plan(&options, XaiVideoProviderOptions::default());
    assert_eq!(p.body["duration"], 10);
    assert_eq!(p.body["resolution"], "720p");
}

#[test]
fn plan_provider_resolution_wins_over_sdk_size() {
    let options = VideoModelV4CallOptions::new("p").with_size(VideoSize::HD720p);
    let xai = xai_video_options(serde_json::json!({ "resolution": "480p" }));
    let p = plan(&options, xai);
    assert_eq!(p.body["resolution"], "480p");
}

#[test]
fn plan_skips_unrecognized_sdk_resolution() {
    let options = VideoModelV4CallOptions::new("p").with_size(VideoSize::UHD4K);
    let p = plan(&options, XaiVideoProviderOptions::default());
    assert!(p.body.get("resolution").is_none());
}

#[test]
fn plan_edit_mode_posts_video_url_and_omits_duration_and_resolution() {
    let options = VideoModelV4CallOptions::new("edit it")
        .with_duration(VideoDuration::Seconds5)
        .with_size(VideoSize::HD720p);
    let xai = xai_video_options(serde_json::json!({
        "mode": "edit-video",
        "videoUrl": "https://example.com/v.mp4",
        "resolution": "480p",
    }));
    let p = plan(&options, xai);
    assert_eq!(p.endpoint, "/videos/edits");
    assert_eq!(
        p.body["video"],
        serde_json::json!({ "url": "https://example.com/v.mp4" })
    );
    assert!(p.body.get("duration").is_none());
    assert!(p.body.get("resolution").is_none());
}

#[test]
fn plan_extend_mode_allows_duration_but_not_resolution() {
    let options = VideoModelV4CallOptions::new("extend it")
        .with_duration(VideoDuration::Seconds5)
        .with_size(VideoSize::HD720p);
    let xai = xai_video_options(serde_json::json!({
        "mode": "extend-video",
        "videoUrl": "https://example.com/v.mp4",
    }));
    let p = plan(&options, xai);
    assert_eq!(p.endpoint, "/videos/extensions");
    assert_eq!(
        p.body["video"],
        serde_json::json!({ "url": "https://example.com/v.mp4" })
    );
    assert_eq!(p.body["duration"], 5);
    assert!(p.body.get("resolution").is_none());
}

#[test]
fn plan_reference_images_go_to_generations() {
    let options = VideoModelV4CallOptions::new("r2v");
    let xai = xai_video_options(serde_json::json!({
        "referenceImageUrls": ["https://example.com/a.png", "https://example.com/b.png"],
    }));
    let p = plan(&options, xai);
    assert_eq!(p.endpoint, "/videos/generations");
    assert_eq!(
        p.body["reference_images"],
        serde_json::json!([
            { "url": "https://example.com/a.png" },
            { "url": "https://example.com/b.png" },
        ])
    );
}

#[test]
fn plan_start_image_becomes_data_uri() {
    let options = VideoModelV4CallOptions::new("i2v").with_image(vec![1, 2, 3], "image/jpeg");
    let p = plan(&options, XaiVideoProviderOptions::default());
    assert_eq!(
        p.body["image"],
        serde_json::json!({ "url": "data:image/jpeg;base64,AQID" })
    );
}

#[test]
fn plan_passes_through_extra_option_keys() {
    let options = VideoModelV4CallOptions::new("p");
    let xai = xai_video_options(serde_json::json!({
        "pollIntervalMs": 10,
        "aspect_ratio": "16:9",
        "custom": { "k": 1 },
    }));
    let p = plan(&options, xai);
    assert_eq!(p.body["aspect_ratio"], "16:9");
    assert_eq!(p.body["custom"], serde_json::json!({ "k": 1 }));
    // Poll controls never reach the wire body.
    assert!(p.body.get("pollIntervalMs").is_none());
}

#[test]
fn map_sdk_resolution_covers_known_dimensions() {
    assert_eq!(map_sdk_resolution("1280x720"), Some("720p"));
    assert_eq!(map_sdk_resolution("854x480"), Some("480p"));
    assert_eq!(map_sdk_resolution("640x480"), Some("480p"));
    assert_eq!(map_sdk_resolution("1920x1080"), None);
}
