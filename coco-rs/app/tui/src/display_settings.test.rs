use std::collections::HashMap;

use coco_config::SettingSource;
use coco_config::Settings;
use coco_config::SettingsWithSource;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

fn settings_with_source(
    merged: Settings,
    per_source: HashMap<SettingSource, serde_json::Value>,
) -> SettingsWithSource {
    SettingsWithSource {
        merged,
        per_source,
        source_paths: HashMap::new(),
    }
}

fn raw_syntax_highlighting(disabled: bool) -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::from_iter([(
        SYNTAX_HIGHLIGHTING_DISABLED_KEY.to_string(),
        json!(disabled),
    )]))
}

#[test]
fn from_settings_with_sources_allows_user_owned_syntax_highlighting() {
    let mut per_source = HashMap::new();
    per_source.insert(SettingSource::User, raw_syntax_highlighting(true));
    let settings = settings_with_source(
        Settings {
            syntax_highlighting_disabled: true,
            ..Settings::default()
        },
        per_source,
    );

    let display = DisplaySettings::from_settings_with_sources(&settings);

    assert_eq!(display.syntax_highlighting, SyntaxHighlighting::Disabled);
    assert!(!display.show_thinking);
    assert_eq!(
        display.syntax_highlighting_editability,
        DisplaySettingEditability::Editable
    );
}

#[test]
fn from_settings_reads_show_thinking_default() {
    let display = DisplaySettings::from_settings(&Settings {
        show_thinking: true,
        ..Settings::default()
    });

    assert!(display.show_thinking);
}

#[test]
fn from_settings_converts_native_replay_cache_kib_to_bytes() {
    let mut settings = Settings::default();
    settings.tui.native_replay_cache.enabled = false;
    settings.tui.native_replay_cache.max_entries = 7;
    settings.tui.native_replay_cache.max_estimated_kb = 128;
    settings.tui.native_replay_cache.min_cells = 3;
    settings.tui.native_replay_cache.min_content_kb = 4;
    settings.tui.native_replay_cache.admit_min_render_us = 99;
    settings.tui.native_replay_cache.admit_min_result_kb = 5;

    let display = DisplaySettings::from_settings(&settings);

    assert!(!display.native_replay_cache.enabled);
    assert_eq!(display.native_replay_cache.max_entries, 7);
    assert_eq!(display.native_replay_cache.max_estimated_bytes, 128 * 1024);
    assert_eq!(display.native_replay_cache.min_cells, 3);
    assert_eq!(display.native_replay_cache.min_content_bytes, 4 * 1024);
    assert_eq!(
        display.native_replay_cache.admit_min_render_elapsed,
        std::time::Duration::from_micros(99)
    );
    assert_eq!(display.native_replay_cache.admit_min_result_bytes, 5 * 1024);
}

#[test]
fn from_settings_converts_tui_performance_defaults_and_overrides() {
    let display = DisplaySettings::from_settings(&Settings::default());

    assert!(!display.performance.enabled);
    assert_eq!(display.performance.sample_every_n_frames, 10);
    assert_eq!(display.performance.slow_frame_ms, 16);
    assert_eq!(display.performance.slow_stage_us, 1000);

    let mut settings = Settings::default();
    settings.tui.performance.enabled = true;
    settings.tui.performance.sample_every_n_frames = 5;
    settings.tui.performance.slow_frame_ms = 24;
    settings.tui.performance.slow_stage_us = 900;

    let display = DisplaySettings::from_settings(&settings);

    assert!(display.performance.enabled);
    assert_eq!(display.performance.sample_every_n_frames, 5);
    assert_eq!(display.performance.slow_frame_ms, 24);
    assert_eq!(display.performance.slow_stage_us, 900);
}

#[test]
fn from_settings_carries_status_line_config() {
    let settings = Settings {
        status_line: Some(coco_config::StatusLineSettings::Command(
            coco_config::StatusLineCommandSettings {
                command: "printf ready".to_string(),
                padding: 0,
            },
        )),
        ..Settings::default()
    };

    let display = DisplaySettings::from_settings(&settings);

    assert_eq!(display.status_line, settings.status_line);
}

#[test]
fn from_settings_with_sources_marks_higher_priority_syntax_highlighting_as_overridden() {
    let mut per_source = HashMap::new();
    per_source.insert(SettingSource::Project, raw_syntax_highlighting(true));
    per_source.insert(SettingSource::Local, raw_syntax_highlighting(false));
    let settings = settings_with_source(Settings::default(), per_source);

    let display = DisplaySettings::from_settings_with_sources(&settings);

    assert_eq!(display.syntax_highlighting, SyntaxHighlighting::Enabled);
    assert_eq!(
        display.syntax_highlighting_editability,
        DisplaySettingEditability::OverriddenBy(SettingSource::Local)
    );
}
