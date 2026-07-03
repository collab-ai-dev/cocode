use super::*;

// Persisting branches write to the process-global user settings.json, so they
// are exercised via integration paths, not here (a unit test must not mutate
// the real config). Only the pure parse/validate/format branches are covered.

#[test]
fn unknown_subcommand_returns_usage_without_persisting() {
    let out = run("wat").expect("ok");
    assert!(out.contains("Unknown subcommand: wat"));
    assert!(out.contains("Usage:"));
}

#[test]
fn invalid_language_is_rejected_without_persisting() {
    let out = run("lang 12345").expect("ok");
    assert!(out.contains("Unsupported language code"));
}

#[test]
fn unknown_backend_is_rejected() {
    let out = run("backend banana").expect("ok");
    assert!(out.contains("Unknown backend: banana"));
}

#[test]
fn backend_without_argument_shows_usage() {
    let out = run("backend").expect("ok");
    assert!(out.contains("/voice-config backend <remote|local>"));
}

#[test]
fn local_without_field_shows_usage() {
    let out = run("local").expect("ok");
    assert!(out.contains("/voice-config local <model|url|base>"));
}

#[test]
fn local_unknown_field_is_rejected() {
    let out = run("local frobnicate x").expect("ok");
    assert!(out.contains("Unknown local field: frobnicate"));
}

#[test]
fn usage_lists_every_subcommand() {
    let text = usage();
    for sub in ["lang", "backend", "remote", "local", "download"] {
        assert!(text.contains(sub), "usage missing `{sub}`: {text}");
    }
}

#[test]
fn whisper_model_path_uses_ggml_bin_naming() {
    let dir = PathBuf::from("/models/whisper");
    let path = whisper_model_path("base.en", Some(&dir));
    assert_eq!(path, PathBuf::from("/models/whisper/ggml-base.en.bin"));
}

#[test]
fn is_valid_language_accepts_codes_and_auto() {
    for good in ["auto", "en", "zh", "pt-br", "zh-hant"] {
        assert!(is_valid_language(good), "should accept {good}");
    }
    for bad in ["", "e", "english", "123", "e-", "toolongsubtag"] {
        assert!(!is_valid_language(bad), "should reject {bad}");
    }
}
