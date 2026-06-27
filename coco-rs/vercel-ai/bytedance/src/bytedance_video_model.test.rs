use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use super::*;

fn make_config() -> ByteDanceVideoModelConfig {
    ByteDanceVideoModelConfig {
        provider: "bytedance".to_string(),
        base_url: "https://ark.ap-southeast.bytepluses.com/api/v3".to_string(),
        headers: Arc::new(HashMap::new),
        client: None,
        poll_interval: None,
        poll_timeout: None,
    }
}

#[test]
fn model_id_and_provider() {
    let model = ByteDanceVideoModel::new("seedance-1-5-pro-251215", make_config());
    assert_eq!(model.model_id(), "seedance-1-5-pro-251215");
    assert_eq!(model.provider(), "bytedance");
}

#[test]
fn default_poll_settings() {
    let model = ByteDanceVideoModel::new("seedance-1-5-pro-251215", make_config());
    assert_eq!(model.poll_interval(), Duration::from_secs(3));
    assert_eq!(model.poll_timeout(), Duration::from_secs(300));
}

#[test]
fn custom_poll_settings() {
    let mut config = make_config();
    config.poll_interval = Some(Duration::from_secs(10));
    config.poll_timeout = Some(Duration::from_secs(600));

    let model = ByteDanceVideoModel::new("seedance-1-5-pro-251215", config);
    assert_eq!(model.poll_interval(), Duration::from_secs(10));
    assert_eq!(model.poll_timeout(), Duration::from_secs(600));
}

#[test]
fn resolution_mapping_1080p() {
    assert_eq!(map_resolution("1920x1080"), "1080p");
    assert_eq!(map_resolution("1080x1920"), "1080p");
    assert_eq!(map_resolution("1664x1248"), "1080p");
    assert_eq!(map_resolution("1248x1664"), "1080p");
    assert_eq!(map_resolution("1440x1440"), "1080p");
    assert_eq!(map_resolution("2206x946"), "1080p");
    assert_eq!(map_resolution("946x2206"), "1080p");
    assert_eq!(map_resolution("1920x1088"), "1080p");
    assert_eq!(map_resolution("1088x1920"), "1080p");
    assert_eq!(map_resolution("2176x928"), "1080p");
    assert_eq!(map_resolution("928x2176"), "1080p");
}

#[test]
fn resolution_mapping_720p() {
    assert_eq!(map_resolution("1280x720"), "720p");
    assert_eq!(map_resolution("720x1280"), "720p");
    assert_eq!(map_resolution("1112x834"), "720p");
    assert_eq!(map_resolution("834x1112"), "720p");
    assert_eq!(map_resolution("960x960"), "720p");
    assert_eq!(map_resolution("1470x630"), "720p");
    assert_eq!(map_resolution("630x1470"), "720p");
    assert_eq!(map_resolution("1248x704"), "720p");
    assert_eq!(map_resolution("704x1248"), "720p");
    assert_eq!(map_resolution("1120x832"), "720p");
    assert_eq!(map_resolution("832x1120"), "720p");
    assert_eq!(map_resolution("1504x640"), "720p");
    assert_eq!(map_resolution("640x1504"), "720p");
}

#[test]
fn resolution_mapping_480p() {
    assert_eq!(map_resolution("864x496"), "480p");
    assert_eq!(map_resolution("496x864"), "480p");
    assert_eq!(map_resolution("752x560"), "480p");
    assert_eq!(map_resolution("560x752"), "480p");
    assert_eq!(map_resolution("640x640"), "480p");
    assert_eq!(map_resolution("992x432"), "480p");
    assert_eq!(map_resolution("432x992"), "480p");
    assert_eq!(map_resolution("864x480"), "480p");
    assert_eq!(map_resolution("480x864"), "480p");
    assert_eq!(map_resolution("736x544"), "480p");
    assert_eq!(map_resolution("544x736"), "480p");
    assert_eq!(map_resolution("960x416"), "480p");
    assert_eq!(map_resolution("416x960"), "480p");
    assert_eq!(map_resolution("832x480"), "480p");
    assert_eq!(map_resolution("480x832"), "480p");
    assert_eq!(map_resolution("624x624"), "480p");
}

#[test]
fn resolution_mapping_passthrough() {
    assert_eq!(map_resolution("3840x2160"), "3840x2160");
    assert_eq!(map_resolution("custom"), "custom");
    assert_eq!(map_resolution("999x999"), "999x999");
}

#[test]
fn parse_provider_options() {
    let json = serde_json::json!({
        "watermark": true,
        "generate_audio": false,
        "camera_fixed": true,
        "return_last_frame": true,
        "service_tier": "flex",
        "draft": false,
        "poll_interval_ms": 5000,
        "poll_timeout_ms": 120000,
    });

    let opts: ByteDanceVideoProviderOptions = serde_json::from_value(json).unwrap();
    assert_eq!(opts.watermark, Some(true));
    assert_eq!(opts.generate_audio, Some(false));
    assert_eq!(opts.camera_fixed, Some(true));
    assert_eq!(opts.return_last_frame, Some(true));
    assert_eq!(opts.service_tier, Some("flex".to_string()));
    assert_eq!(opts.draft, Some(false));
    assert_eq!(opts.poll_interval_ms, Some(5000));
    assert_eq!(opts.poll_timeout_ms, Some(120000));
}

#[test]
fn parse_provider_options_defaults() {
    let json = serde_json::json!({});
    let opts: ByteDanceVideoProviderOptions = serde_json::from_value(json).unwrap();
    assert_eq!(opts.watermark, None);
    assert_eq!(opts.generate_audio, None);
    assert_eq!(opts.service_tier, None);
    assert_eq!(opts.poll_interval_ms, None);
}
