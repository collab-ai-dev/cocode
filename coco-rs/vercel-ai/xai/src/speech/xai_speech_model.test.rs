use super::*;
use pretty_assertions::assert_eq;
use vercel_ai_provider::ProviderOptions;

fn model() -> XaiSpeechModel {
    let config = Arc::new(XaiConfig {
        provider: "xai.speech".into(),
        base_url: "https://api.x.ai/v1".into(),
        headers: Arc::new(HashMap::new),
        client: None,
    });
    XaiSpeechModel::new("", config)
}

fn xai_speech_options(value: serde_json::Value) -> XaiSpeechProviderOptions {
    let map: HashMap<String, serde_json::Value> =
        serde_json::from_value(value).expect("options map");
    let mut po = ProviderOptions::default();
    po.0.insert("xai".into(), map);
    extract_xai_speech_options(&Some(po))
}

#[test]
fn model_exposes_provider_and_empty_id() {
    let m = model();
    assert_eq!(m.provider(), "xai.speech");
    assert_eq!(m.model_id(), "");
}

#[test]
fn plan_uses_xai_defaults() {
    let options = SpeechModelV4CallOptions::new("hello world");
    let plan = plan_speech_request(&options, &XaiSpeechProviderOptions::default());
    assert_eq!(
        plan.body,
        serde_json::json!({
            "text": "hello world",
            "voice_id": "eve",
            "language": "auto",
            "output_format": { "codec": "mp3" },
        })
    );
    assert_eq!(plan.codec, "mp3");
    assert!(plan.warnings.is_empty());
}

#[test]
fn plan_passes_standard_options() {
    let options = SpeechModelV4CallOptions::new("hi")
        .with_voice("ara")
        .with_output_format("wav")
        .with_speed(1.5)
        .with_language("en");
    let plan = plan_speech_request(&options, &XaiSpeechProviderOptions::default());
    assert_eq!(plan.body["voice_id"], "ara");
    assert_eq!(plan.body["language"], "en");
    assert_eq!(
        plan.body["output_format"],
        serde_json::json!({ "codec": "wav" })
    );
    assert_eq!(plan.body["speed"], 1.5);
}

#[test]
fn plan_maps_provider_options_onto_request_fields() {
    let options = SpeechModelV4CallOptions::new("hi");
    let xai = xai_speech_options(serde_json::json!({
        "sampleRate": 44100,
        "bitRate": 128000,
        "optimizeStreamingLatency": 2,
        "textNormalization": true,
    }));
    let plan = plan_speech_request(&options, &xai);
    assert_eq!(
        plan.body["output_format"],
        serde_json::json!({ "codec": "mp3", "sample_rate": 44100, "bit_rate": 128000 })
    );
    assert_eq!(plan.body["optimize_streaming_latency"], 2);
    assert_eq!(plan.body["text_normalization"], true);
}

#[test]
fn plan_warns_and_falls_back_to_mp3_for_unknown_format() {
    let options = SpeechModelV4CallOptions::new("hi").with_output_format("ogg");
    let plan = plan_speech_request(&options, &XaiSpeechProviderOptions::default());
    assert_eq!(plan.codec, "mp3");
    assert_eq!(plan.body["output_format"]["codec"], "mp3");
    assert_eq!(plan.warnings.len(), 1);
    match &plan.warnings[0] {
        Warning::Unsupported { feature, details } => {
            assert_eq!(feature, "outputFormat");
            assert!(details.as_deref().unwrap_or("").contains("ogg"));
        }
        other => panic!("unexpected warning: {other:?}"),
    }
}

#[test]
fn plan_warns_and_ignores_bit_rate_for_non_mp3() {
    let options = SpeechModelV4CallOptions::new("hi").with_output_format("wav");
    let xai = xai_speech_options(serde_json::json!({ "bitRate": 128000 }));
    let plan = plan_speech_request(&options, &xai);
    assert!(plan.body["output_format"].get("bit_rate").is_none());
    assert_eq!(plan.warnings.len(), 1);
}

#[test]
fn plan_warns_on_instructions() {
    let options = SpeechModelV4CallOptions::new("hi").with_instructions("be dramatic");
    let plan = plan_speech_request(&options, &XaiSpeechProviderOptions::default());
    assert_eq!(plan.warnings.len(), 1);
    match &plan.warnings[0] {
        Warning::Unsupported { feature, .. } => assert_eq!(feature, "instructions"),
        other => panic!("unexpected warning: {other:?}"),
    }
    // Instructions never reach the wire body.
    assert!(plan.body.get("instructions").is_none());
}

#[test]
fn codec_to_content_type_covers_all_codecs() {
    assert_eq!(codec_to_content_type("mp3"), "audio/mpeg");
    assert_eq!(codec_to_content_type("wav"), "audio/wav");
    assert_eq!(codec_to_content_type("pcm"), "audio/pcm");
    assert_eq!(codec_to_content_type("mulaw"), "audio/mulaw");
    assert_eq!(codec_to_content_type("alaw"), "audio/alaw");
}
