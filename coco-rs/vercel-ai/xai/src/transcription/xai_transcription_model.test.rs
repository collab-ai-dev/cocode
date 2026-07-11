use super::*;
use pretty_assertions::assert_eq;
use vercel_ai_provider::ProviderOptions;

fn model() -> XaiTranscriptionModel {
    let config = Arc::new(XaiConfig {
        provider: "xai.transcription".into(),
        base_url: "https://api.x.ai/v1".into(),
        headers: Arc::new(HashMap::new),
        client: None,
    });
    XaiTranscriptionModel::new("", config)
}

fn xai_transcription_options(value: serde_json::Value) -> XaiTranscriptionProviderOptions {
    let map: HashMap<String, serde_json::Value> =
        serde_json::from_value(value).expect("options map");
    let mut po = ProviderOptions::default();
    po.0.insert("xai".into(), map);
    extract_xai_transcription_options(&Some(po))
}

#[test]
fn model_exposes_provider_and_empty_id() {
    let m = model();
    assert_eq!(m.provider(), "xai.transcription");
    assert_eq!(m.model_id(), "");
}

#[test]
fn plan_is_empty_without_options() {
    let fields = plan_transcription_fields(&XaiTranscriptionProviderOptions::default());
    assert!(fields.is_empty());
}

#[test]
fn plan_maps_provider_options_in_wire_order() {
    let xai = xai_transcription_options(serde_json::json!({
        "audioFormat": "pcm",
        "sampleRate": 16000,
        "language": "en",
        "format": true,
        "multichannel": true,
        "channels": 2,
        "diarize": false,
        "fillerWords": true,
    }));
    let fields = plan_transcription_fields(&xai);
    assert_eq!(
        fields,
        vec![
            ("audio_format", "pcm".to_string()),
            ("sample_rate", "16000".to_string()),
            ("language", "en".to_string()),
            ("format", "true".to_string()),
            ("multichannel", "true".to_string()),
            ("channels", "2".to_string()),
            ("diarize", "false".to_string()),
            ("filler_words", "true".to_string()),
        ]
    );
}

#[test]
fn plan_repeats_keyterm_for_list_values() {
    let xai = xai_transcription_options(serde_json::json!({
        "keyterm": ["Grok", "xAI"],
    }));
    let fields = plan_transcription_fields(&xai);
    assert_eq!(
        fields,
        vec![
            ("keyterm", "Grok".to_string()),
            ("keyterm", "xAI".to_string()),
        ]
    );
}

#[test]
fn plan_accepts_single_keyterm_string() {
    let xai = xai_transcription_options(serde_json::json!({ "keyterm": "Grok" }));
    let fields = plan_transcription_fields(&xai);
    assert_eq!(fields, vec![("keyterm", "Grok".to_string())]);
}

#[test]
fn extension_from_media_type_mirrors_ts_helper() {
    assert_eq!(extension_from_media_type("audio/mpeg"), "mp3");
    assert_eq!(extension_from_media_type("audio/x-wav"), "wav");
    assert_eq!(extension_from_media_type("audio/opus"), "ogg");
    assert_eq!(extension_from_media_type("audio/mp4"), "m4a");
    assert_eq!(extension_from_media_type("audio/x-m4a"), "m4a");
    assert_eq!(extension_from_media_type("audio/wav"), "wav");
    assert_eq!(extension_from_media_type("AUDIO/FLAC"), "flac");
}
