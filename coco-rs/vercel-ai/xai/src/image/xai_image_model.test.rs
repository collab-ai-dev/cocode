use super::*;
use pretty_assertions::assert_eq;
use std::collections::HashMap;
use vercel_ai_provider::ImageSize;
use vercel_ai_provider::ProviderOptions;

fn model() -> XaiImageModel {
    let config = Arc::new(XaiConfig {
        provider: "xai.image".into(),
        base_url: "https://api.x.ai/v1".into(),
        headers: Arc::new(HashMap::new),
        client: None,
    });
    XaiImageModel::new("grok-imagine-image", config)
}

fn xai_options(value: serde_json::Value) -> Option<ProviderOptions> {
    let map: HashMap<String, serde_json::Value> =
        serde_json::from_value(value).expect("options map");
    let mut po = ProviderOptions::default();
    po.0.insert("xai".into(), map);
    Some(po)
}

#[test]
fn model_exposes_provider_and_id() {
    let m = model();
    assert_eq!(m.provider(), "xai.image");
    assert_eq!(m.model_id(), "grok-imagine-image");
    assert_eq!(m.max_images_per_call(), 3);
}

#[test]
fn plan_defaults_to_generations_with_b64_json() {
    let options = ImageModelV4CallOptions::new("a cat").with_n(2);
    let plan = plan_image_request("grok-imagine-image", &options, &Default::default());
    assert_eq!(plan.endpoint, "/images/generations");
    assert_eq!(
        plan.body,
        serde_json::json!({
            "model": "grok-imagine-image",
            "prompt": "a cat",
            "n": 2,
            "response_format": "b64_json",
        })
    );
    assert!(plan.warnings.is_empty());
}

#[test]
fn plan_warns_on_size_seed_and_mask() {
    let options = ImageModelV4CallOptions::new("a cat")
        .with_size(ImageSize::S1024x1024)
        .with_seed(42)
        .with_mask(ImageModelV4File::Url {
            url: "https://example.com/mask.png".into(),
            provider_options: None,
        });
    let plan = plan_image_request("grok-imagine-image", &options, &Default::default());
    let features: Vec<&str> = plan
        .warnings
        .iter()
        .map(|w| match w {
            Warning::Unsupported { feature, .. } => feature.as_str(),
            Warning::Compatibility { feature, .. } => feature.as_str(),
            Warning::Other { message } => message.as_str(),
            other => panic!("unexpected warning: {other:?}"),
        })
        .collect();
    assert_eq!(features, vec!["size", "seed", "mask"]);
    // Neither size, seed, nor mask reach the wire body.
    assert!(plan.body.get("size").is_none());
    assert!(plan.body.get("seed").is_none());
    assert!(plan.body.get("mask").is_none());
}

#[test]
fn plan_top_level_aspect_ratio_wins_over_provider_option() {
    let options = ImageModelV4CallOptions::new("a cat").with_aspect_ratio("16:9");
    let xai = extract_xai_image_options(&xai_options(serde_json::json!({
        "aspect_ratio": "1:1",
    })));
    let plan = plan_image_request("grok-imagine-image", &options, &xai);
    assert_eq!(plan.body["aspect_ratio"], "16:9");
}

#[test]
fn plan_applies_provider_options() {
    let options = ImageModelV4CallOptions::new("a cat");
    let xai = extract_xai_image_options(&xai_options(serde_json::json!({
        "aspect_ratio": "4:3",
        "output_format": "png",
        "sync_mode": true,
        "resolution": "2k",
        "quality": "high",
        "user": "user-1",
    })));
    let plan = plan_image_request("grok-imagine-image", &options, &xai);
    assert_eq!(plan.body["aspect_ratio"], "4:3");
    assert_eq!(plan.body["output_format"], "png");
    assert_eq!(plan.body["sync_mode"], true);
    assert_eq!(plan.body["resolution"], "2k");
    assert_eq!(plan.body["quality"], "high");
    assert_eq!(plan.body["user"], "user-1");
}

#[test]
fn plan_single_file_routes_to_edits_with_image_object() {
    let options = ImageModelV4CallOptions::new("edit it").with_files(vec![ImageModelV4File::Url {
        url: "https://example.com/in.png".into(),
        provider_options: None,
    }]);
    let plan = plan_image_request("grok-imagine-image", &options, &Default::default());
    assert_eq!(plan.endpoint, "/images/edits");
    assert_eq!(
        plan.body["image"],
        serde_json::json!({ "url": "https://example.com/in.png", "type": "image_url" })
    );
    assert!(plan.body.get("images").is_none());
}

#[test]
fn plan_multiple_files_use_images_array_and_data_uris() {
    let options = ImageModelV4CallOptions::new("combine").with_files(vec![
        ImageModelV4File::Url {
            url: "https://example.com/a.png".into(),
            provider_options: None,
        },
        ImageModelV4File::File {
            media_type: "image/png".into(),
            data: ImageFileData::Base64("QUJD".into()),
            provider_options: None,
        },
        ImageModelV4File::File {
            media_type: "image/jpeg".into(),
            data: ImageFileData::Binary(vec![1, 2, 3]),
            provider_options: None,
        },
    ]);
    let plan = plan_image_request("grok-imagine-image", &options, &Default::default());
    assert_eq!(plan.endpoint, "/images/edits");
    assert_eq!(
        plan.body["images"],
        serde_json::json!([
            { "url": "https://example.com/a.png", "type": "image_url" },
            { "url": "data:image/png;base64,QUJD", "type": "image_url" },
            { "url": "data:image/jpeg;base64,AQID", "type": "image_url" },
        ])
    );
    assert!(plan.body.get("image").is_none());
    // Multiple files must not produce warnings.
    assert!(plan.warnings.is_empty());
}

#[test]
fn provider_metadata_carries_revised_prompt_and_cost() {
    let response = XaiImageResponse {
        data: vec![
            XaiImageData {
                url: None,
                b64_json: Some("Zm9v".into()),
                revised_prompt: Some("a fluffy cat".into()),
            },
            XaiImageData {
                url: None,
                b64_json: Some("YmFy".into()),
                revised_prompt: None,
            },
        ],
        usage: Some(XaiImageUsage {
            cost_in_usd_ticks: Some(1234),
        }),
    };
    let meta = build_provider_metadata(&response);
    assert_eq!(
        meta.0.get("xai"),
        Some(&serde_json::json!({
            "images": [ { "revisedPrompt": "a fluffy cat" }, {} ],
            "costInUsdTicks": 1234,
        }))
    );
}
