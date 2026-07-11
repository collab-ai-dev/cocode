use super::*;
use crate::transcription::xai_transcription_options::XaiStreamingOptions;
use crate::transcription::xai_transcription_options::XaiTranscriptionAudioFormat;
use pretty_assertions::assert_eq;

fn parse_query(url: &str) -> std::collections::HashMap<String, Vec<String>> {
    let parsed = reqwest::Url::parse(url).expect("valid url");
    let mut map: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    for (k, v) in parsed.query_pairs() {
        map.entry(k.into_owned()).or_default().push(v.into_owned());
    }
    map
}

#[test]
fn to_ws_scheme_maps_http_schemes() {
    assert_eq!(to_ws_scheme("https://api.x.ai/v1"), "wss://api.x.ai/v1");
    assert_eq!(
        to_ws_scheme("http://localhost:8080/v1"),
        "ws://localhost:8080/v1"
    );
}

#[test]
fn encoding_defaults_from_media_type() {
    assert_eq!(encoding_from_media_type("audio/pcmu"), "mulaw");
    assert_eq!(encoding_from_media_type("audio/pcma"), "alaw");
    assert_eq!(encoding_from_media_type("audio/pcm"), "pcm");
    assert_eq!(encoding_from_media_type("audio/anything-else"), "pcm");
}

#[test]
fn build_url_uses_wss_and_defaults_sample_rate_and_encoding() {
    let fmt = TranscriptionInputAudioFormat::new("audio/pcmu", Some(8000));
    let opts = XaiTranscriptionProviderOptions::default();
    let url = build_streaming_url("https://api.x.ai/v1", &fmt, &opts).expect("url");
    assert!(url.starts_with("wss://api.x.ai/v1/stt?"), "got {url}");
    let q = parse_query(&url);
    // sample_rate falls back to the input format rate; encoding to mulaw.
    assert_eq!(q["sample_rate"], vec!["8000"]);
    assert_eq!(q["encoding"], vec!["mulaw"]);
}

#[test]
fn build_url_options_override_and_streaming_params_and_repeated_keyterm() {
    let fmt = TranscriptionInputAudioFormat::new("audio/pcm", Some(16000));
    let opts = XaiTranscriptionProviderOptions {
        audio_format: Some(XaiTranscriptionAudioFormat::Alaw),
        sample_rate: Some(48000),
        language: Some("en".to_string()),
        diarize: Some(true),
        filler_words: Some(false),
        multichannel: Some(true),
        channels: Some(2),
        keyterm: Some(
            crate::transcription::xai_transcription_options::XaiKeyterm::Many(vec![
                "xAI".to_string(),
                "Grok".to_string(),
            ]),
        ),
        streaming: Some(XaiStreamingOptions {
            interim_results: Some(true),
            endpointing: Some(300),
            smart_turn: Some(0.5),
            smart_turn_timeout: Some(2000),
        }),
        ..Default::default()
    };
    let url = build_streaming_url("https://api.x.ai/v1", &fmt, &opts).expect("url");
    let q = parse_query(&url);
    // Explicit options win over the input-format fallbacks.
    assert_eq!(q["sample_rate"], vec!["48000"]);
    assert_eq!(q["encoding"], vec!["alaw"]);
    assert_eq!(q["language"], vec!["en"]);
    assert_eq!(q["diarize"], vec!["true"]);
    assert_eq!(q["filler_words"], vec!["false"]);
    assert_eq!(q["multichannel"], vec!["true"]);
    assert_eq!(q["channels"], vec!["2"]);
    assert_eq!(q["interim_results"], vec!["true"]);
    assert_eq!(q["endpointing"], vec!["300"]);
    assert_eq!(q["smart_turn"], vec!["0.5"]);
    assert_eq!(q["smart_turn_timeout"], vec!["2000"]);
    // keyterm repeats.
    assert_eq!(q["keyterm"], vec!["xAI", "Grok"]);
}

#[test]
fn build_partial_selects_final_on_is_final_with_timing() {
    let raw = serde_json::json!({
        "type": "transcript.partial",
        "text": "hello world",
        "start": 1.0,
        "duration": 0.5,
        "channel_index": 0,
        "is_final": true
    });
    match build_partial(&raw) {
        TranscriptionModelV4StreamPart::TranscriptFinal {
            id,
            text,
            start_second,
            end_second,
            channel_index,
            ..
        } => {
            assert_eq!(id.as_deref(), Some("channel-0"));
            assert_eq!(text, "hello world");
            assert_eq!(start_second, Some(1.0));
            assert_eq!(end_second, Some(1.5));
            assert_eq!(channel_index, Some(0));
        }
        other => panic!("expected transcript-final, got {other:?}"),
    }
}

#[test]
fn build_partial_is_partial_when_not_final() {
    let raw = serde_json::json!({
        "type": "transcript.partial",
        "text": "hel",
        "start": 0.0,
        "duration": 0.2
    });
    match build_partial(&raw) {
        TranscriptionModelV4StreamPart::TranscriptPartial {
            id,
            text,
            duration_in_seconds,
            ..
        } => {
            assert_eq!(id, None);
            assert_eq!(text, "hel");
            assert_eq!(duration_in_seconds, Some(0.2));
        }
        other => panic!("expected transcript-partial, got {other:?}"),
    }
}
