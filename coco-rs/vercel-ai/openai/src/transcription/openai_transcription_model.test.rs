use super::*;

#[test]
fn map_language_code_known_languages() {
    assert_eq!(map_language_code("english"), Some("en"));
    assert_eq!(map_language_code("french"), Some("fr"));
    assert_eq!(map_language_code("german"), Some("de"));
    assert_eq!(map_language_code("japanese"), Some("ja"));
    assert_eq!(map_language_code("chinese"), Some("zh"));
    assert_eq!(map_language_code("spanish"), Some("es"));
    assert_eq!(map_language_code("arabic"), Some("ar"));
    assert_eq!(map_language_code("korean"), Some("ko"));
    assert_eq!(map_language_code("portuguese"), Some("pt"));
    assert_eq!(map_language_code("russian"), Some("ru"));
    assert_eq!(map_language_code("welsh"), Some("cy"));
}

#[test]
fn map_language_code_unknown_returns_none() {
    assert_eq!(map_language_code("klingon"), None);
    assert_eq!(map_language_code(""), None);
    assert_eq!(map_language_code("English"), None); // case-sensitive
}

#[test]
fn extension_from_media_type_known() {
    assert_eq!(extension_from_media_type("audio/wav"), "wav");
    assert_eq!(extension_from_media_type("audio/mp3"), "mp3");
    assert_eq!(extension_from_media_type("audio/mpeg"), "mp3");
    assert_eq!(extension_from_media_type("audio/mp4"), "m4a");
    assert_eq!(extension_from_media_type("audio/webm"), "webm");
    assert_eq!(extension_from_media_type("audio/ogg"), "ogg");
    assert_eq!(extension_from_media_type("audio/flac"), "flac");
}

#[test]
fn extension_from_media_type_unknown_returns_bin() {
    assert_eq!(extension_from_media_type("audio/unknown"), "bin");
    assert_eq!(extension_from_media_type("video/mp4"), "bin");
}

#[test]
fn json_response_format_models_exact_match() {
    // Exact matches should use "json"
    assert!(JSON_RESPONSE_FORMAT_MODELS.contains(&"gpt-4o-transcribe"));
    assert!(JSON_RESPONSE_FORMAT_MODELS.contains(&"gpt-4o-mini-transcribe"));

    // Prefix-extended model IDs should NOT match (exact match behavior)
    assert!(!JSON_RESPONSE_FORMAT_MODELS.contains(&"gpt-4o-transcribe-v2"));
    assert!(!JSON_RESPONSE_FORMAT_MODELS.contains(&"gpt-4o-mini-transcribe-beta"));
}
