use super::*;

use pretty_assertions::assert_eq;

fn cfg(model: &str) -> LocalWhisperConfig {
    LocalWhisperConfig {
        model: model.to_string(),
        ..Default::default()
    }
}

#[test]
fn known_models_are_well_formed() {
    assert!(!KNOWN_MODELS.is_empty());
    for m in KNOWN_MODELS {
        assert_eq!(
            m.file,
            format!("ggml-{}.bin", m.name),
            "file naming for {}",
            m.name
        );
        assert_eq!(m.sha256.len(), 64, "sha256 length for {}", m.name);
        assert!(
            m.sha256
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "sha256 must be lowercase hex for {}",
            m.name
        );
        assert!(m.size > 0, "size for {}", m.name);
    }
}

#[test]
fn find_model_hits_and_misses() {
    assert_eq!(find_model("base.en").map(|m| m.name), Some("base.en"));
    assert!(find_model("does-not-exist").is_none());
}

#[test]
fn model_file_name_known_and_unknown() {
    assert_eq!(model_file_name("small"), "ggml-small.bin");
    // Unknown model falls back to the conventional naming.
    assert_eq!(model_file_name("my-custom"), "ggml-my-custom.bin");
}

#[test]
fn resolve_model_path_uses_cache_dir_and_file() {
    let mut c = cfg("base.en");
    c.cache_dir = Some(PathBuf::from("/models/whisper"));
    assert_eq!(
        resolve_model_path(&c),
        PathBuf::from("/models/whisper/ggml-base.en.bin")
    );
}

#[test]
fn resolve_download_url_priority_and_trailing_slash() {
    // Default base.
    assert_eq!(
        resolve_download_url(&cfg("base.en")),
        format!("{DEFAULT_DOWNLOAD_BASE}/ggml-base.en.bin")
    );

    // Mirror base (trailing slash trimmed).
    let mut mirror = cfg("small");
    mirror.download_base =
        Some("https://hf-mirror.com/ggerganov/whisper.cpp/resolve/main/".to_string());
    assert_eq!(
        resolve_download_url(&mirror),
        "https://hf-mirror.com/ggerganov/whisper.cpp/resolve/main/ggml-small.bin"
    );

    // Full URL override wins over everything.
    let mut full = cfg("base.en");
    full.model_url = Some("https://example.test/weights.bin".to_string());
    full.download_base = Some("https://ignored.test".to_string());
    assert_eq!(
        resolve_download_url(&full),
        "https://example.test/weights.bin"
    );
}

#[test]
fn may_auto_download_only_for_pinned_known_models() {
    assert!(may_auto_download(&cfg("base.en")));

    // Unknown model: no pinned checksum → no silent auto-download.
    assert!(!may_auto_download(&cfg("mystery")));

    // Custom URL disables auto-download even for a known name.
    let mut custom = cfg("base.en");
    custom.model_url = Some("https://example.test/x.bin".to_string());
    assert!(!may_auto_download(&custom));

    // auto_download off.
    let mut off = cfg("base.en");
    off.auto_download = false;
    assert!(!may_auto_download(&off));
}

#[test]
fn build_download_request_pins_known_but_not_custom_url() {
    let known = build_download_request(&cfg("base.en"), "ua/1".to_string());
    let spec = find_model("base.en").unwrap();
    assert_eq!(known.expected_sha256.as_deref(), Some(spec.sha256));
    assert_eq!(known.expected_size, Some(spec.size));
    assert_eq!(known.user_agent, "ua/1");

    // A custom model_url downloads unverified.
    let mut custom = cfg("base.en");
    custom.model_url = Some("https://example.test/x.bin".to_string());
    let req = build_download_request(&custom, "ua/1".to_string());
    assert_eq!(req.url, "https://example.test/x.bin");
    assert!(req.expected_sha256.is_none());
    assert!(req.expected_size.is_none());
}
