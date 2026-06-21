//! Tests for the keymap source-of-truth. These guard:
//!
//! - **i18n key resolution**: every `description_key` resolves in both
//!   `en` and `zh-CN`. Without this guard, a typo would surface as the
//!   raw key (e.g. `keymap.input.cursor_home`) in the user's terminal.
//! - **id uniqueness**: `KeymapEntry::id` is used by the JSON export
//!   consumer; duplicates would silently merge.
//! - **verb id whitelist**: `KeymapBinding::Verb { id }` references a
//!   known `TuiCommand` verb so we don't ship a help screen advertising
//!   a verb that doesn't exist.
//! - **group coverage**: every entry lands in one of the declared
//!   `GROUP_ORDER` groups (no orphans).
//! - **JSON export shape**: the JSON dump deserializes back to entries
//!   the subagent can consume.

use std::collections::HashSet;

use super::*;
use crate::i18n::locale_test_guard;
use rust_i18n::set_locale;

#[test]
fn ids_are_unique() {
    let mut seen = HashSet::new();
    for entry in KEYMAP {
        assert!(
            seen.insert(entry.id),
            "duplicate keymap entry id: {}",
            entry.id
        );
    }
}

#[test]
fn every_description_key_resolves_en() {
    let _locale = locale_test_guard("en");
    assert_all_resolve();
}

#[test]
fn every_description_key_resolves_zh() {
    let _locale = locale_test_guard("zh-CN");
    assert_all_resolve();
}

#[test]
fn every_group_title_resolves_en() {
    let _locale = locale_test_guard("en");
    for &group in GROUP_ORDER {
        let key = group.title_key();
        let value = crate::i18n::t!(key).to_string();
        assert_ne!(value, key, "group title `{key}` does not resolve in en");
    }
}

#[test]
fn every_group_title_resolves_zh() {
    let _locale = locale_test_guard("zh-CN");
    for &group in GROUP_ORDER {
        let key = group.title_key();
        let value = crate::i18n::t!(key).to_string();
        assert_ne!(value, key, "group title `{key}` does not resolve in zh-CN");
    }
}

fn assert_all_resolve() {
    for entry in KEYMAP {
        let key = entry.description_key;
        let value = crate::i18n::t!(key).to_string();
        let locale = rust_i18n::locale().to_string();
        assert_ne!(
            value, key,
            "i18n key `{key}` (entry `{}`) does not resolve in `{locale}`",
            entry.id,
        );
    }
}

#[test]
fn every_verb_id_is_known() {
    // The bridge dispatches `KeyEvent` → `TuiCommand`. Verb ids in the
    // keymap reference those `TuiCommand` variants in `snake_case`.
    // Keep this list in lockstep with `keybinding_bridge::map_input_key`.
    let known: HashSet<&'static str> = [
        "cursor_home",
        "cursor_end",
        "cursor_left",
        "cursor_right",
        "word_left",
        "word_right",
        "history",
        "delete_backward",
        "delete_forward",
        "delete_word_backward",
        "delete_word_forward",
        "kill_to_end_of_line",
        "kill_to_beginning_of_line",
        "yank",
        "insert_newline",
        "submit_input",
        "cancel",
        "quit",
    ]
    .into_iter()
    .collect();

    for entry in KEYMAP {
        if let KeymapBinding::Verb { id } = entry.binding {
            assert!(
                known.contains(id),
                "verb id `{id}` (entry `{}`) is not in the bridge whitelist — \
                 add the verb to `keybinding_bridge::map_input_key` and to \
                 this test's `known` set",
                entry.id,
            );
        }
    }
}

#[test]
fn every_entry_belongs_to_declared_group() {
    let declared: HashSet<KeymapGroup> = GROUP_ORDER.iter().copied().collect();
    for entry in KEYMAP {
        assert!(
            declared.contains(&entry.group),
            "entry `{}` uses group not in GROUP_ORDER",
            entry.id,
        );
    }
}

#[test]
fn group_order_covers_every_used_group() {
    let used: HashSet<KeymapGroup> = KEYMAP.iter().map(|e| e.group).collect();
    for group in used {
        assert!(
            GROUP_ORDER.contains(&group),
            "group `{group:?}` is used by KEYMAP but missing from GROUP_ORDER"
        );
    }
}

#[test]
fn entries_for_group_returns_only_that_group() {
    for &group in GROUP_ORDER {
        for entry in entries_for_group(group) {
            assert_eq!(entry.group, group);
        }
    }
}

#[test]
fn export_json_round_trips() {
    let _locale = locale_test_guard("en");
    let json = export_json();
    let parsed: serde_json::Value =
        serde_json::from_str(&json).expect("export_json must produce valid JSON");
    let arr = parsed.as_array().expect("top-level must be an array");
    assert_eq!(arr.len(), KEYMAP.len(), "JSON length must match KEYMAP");
    // Spot-check shape on first entry.
    let first = &arr[0];
    assert!(first.get("id").is_some());
    assert!(first.get("combo").is_some());
    assert!(first.get("group").is_some());
    assert!(first.get("binding").is_some());
    assert!(first.get("description").is_some());
}

#[test]
fn export_markdown_contains_every_combo() {
    let _locale = locale_test_guard("en");
    let md = export_markdown();
    for entry in KEYMAP {
        assert!(
            md.contains(entry.combo),
            "markdown export missing combo `{}` for entry `{}`",
            entry.combo,
            entry.id,
        );
    }
}

#[test]
fn export_markdown_uses_locale() {
    let _locale = locale_test_guard("en");
    set_locale("zh-CN");
    let zh = export_markdown();
    set_locale("en");
    let en = export_markdown();
    assert_ne!(zh, en, "markdown export must differ between locales");
}

#[test]
fn combos_iterator_yields_primary_then_alternates() {
    let entry = KEYMAP
        .iter()
        .find(|e| e.id == "input:cursor_home")
        .expect("input:cursor_home must exist");
    let combos: Vec<&str> = entry.combos().collect();
    assert_eq!(combos, vec!["Ctrl+A", "Home"]);
}
