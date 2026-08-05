use std::collections::HashSet;

use super::*;
use crate::i18n::locale_test_guard;

#[test]
fn test_registry_keys_are_unique_and_non_empty() {
    let mut keys = HashSet::new();
    let ids = settings().iter().map(|meta| meta.id).collect::<Vec<_>>();
    assert_eq!(ids, SettingId::ALL);
    for meta in settings() {
        assert!(!meta.id.key().is_empty());
        assert!(
            keys.insert(meta.id.key()),
            "duplicate key: {}",
            meta.id.key()
        );
    }
}

#[test]
fn test_registry_searches_keys_and_visible_english_metadata() {
    let _locale = locale_test_guard("en");
    let syntax = settings()
        .iter()
        .find(|meta| meta.id == SettingId::SyntaxHighlighting)
        .expect("syntax setting");
    assert!(matches(syntax, "highlight"));
    assert!(matches(syntax, "SYNTAX"));
    assert!(matches(syntax, "syntax highlighting"));
    assert!(matches(syntax, "appearance"));
    assert!(!matches(syntax, "clipboard"));
}

#[test]
fn test_registry_searches_visible_localized_metadata() {
    let _locale = locale_test_guard("zh-CN");
    let syntax = settings()
        .iter()
        .find(|meta| meta.id == SettingId::SyntaxHighlighting)
        .expect("syntax setting");

    assert!(matches(syntax, "高亮"));
    assert!(matches(syntax, "外观"));
    assert!(matches(syntax, "精简"));
}

#[test]
fn test_registry_uses_canonical_status_line_key_and_tracks_alias() {
    assert_eq!(SettingId::StatusLine.key(), "statusLine");
    assert_eq!(
        SettingId::StatusLine.source_keys(),
        &["statusLine", "status_line"]
    );
}

#[test]
fn test_registry_kind_metadata_is_explicit() {
    assert!(
        settings()
            .iter()
            .any(|meta| meta.kind == SettingKind::Boolean)
    );
    assert!(
        settings()
            .iter()
            .any(|meta| meta.kind == SettingKind::Choice)
    );
    assert!(
        settings()
            .iter()
            .any(|meta| meta.kind == SettingKind::Sequence)
    );
    assert!(
        settings()
            .iter()
            .any(|meta| meta.kind == SettingKind::Command)
    );
    assert_eq!(
        settings()
            .iter()
            .find(|meta| meta.id == SettingId::StatusLine)
            .expect("status line setting")
            .kind,
        SettingKind::Command
    );
    assert!(settings().iter().all(|meta| {
        meta.keywords
            .iter()
            .all(|keyword| !keyword.is_empty() && *keyword == keyword.to_lowercase())
    }));
}
