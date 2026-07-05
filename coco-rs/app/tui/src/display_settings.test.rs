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

fn raw_syntax_highlighting(level: SyntaxHighlightingLevel) -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::from_iter([(
        SYNTAX_HIGHLIGHTING_KEY.to_string(),
        json!(level),
    )]))
}

#[test]
fn from_settings_with_sources_allows_user_owned_syntax_highlighting() {
    let mut per_source = HashMap::new();
    per_source.insert(
        SettingSource::User,
        raw_syntax_highlighting(SyntaxHighlightingLevel::Off),
    );
    let settings = settings_with_source(
        Settings {
            syntax_highlighting: SyntaxHighlightingLevel::Off,
            ..Settings::default()
        },
        per_source,
    );

    let display = DisplaySettings::from_settings_with_sources(&settings);

    assert_eq!(display.syntax_highlighting, SyntaxHighlighting::Off);
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

    assert!(!display.performance.frame_enabled);
    assert_eq!(display.performance.frame_sample_every_n_frames, 10);
    assert_eq!(display.performance.frame_slow_threshold_ms, 16);
    assert_eq!(display.performance.frame_stage_slow_threshold_us, 1000);
    assert!(!display.performance.memory_enabled);
    assert_eq!(display.performance.memory_sample_interval_secs, 30);
    assert_eq!(
        display.performance.memory_delta_threshold_bytes,
        4 * 1024 * 1024
    );
    assert!(!display.performance.heap_profile_enabled);

    let mut settings = Settings::default();
    settings.tui.performance.frame_enabled = true;
    settings.tui.performance.frame_sample_every_n_frames = 5;
    settings.tui.performance.frame_slow_threshold_ms = 24;
    settings.tui.performance.frame_stage_slow_threshold_us = 900;
    settings.tui.performance.memory_enabled = true;
    settings.tui.performance.memory_sample_interval_secs = 0;
    settings.tui.performance.memory_delta_threshold_mb = 0;
    settings.tui.performance.heap_profile_enabled = true;

    let display = DisplaySettings::from_settings(&settings);

    assert!(display.performance.frame_enabled);
    assert_eq!(display.performance.frame_sample_every_n_frames, 5);
    assert_eq!(display.performance.frame_slow_threshold_ms, 24);
    assert_eq!(display.performance.frame_stage_slow_threshold_us, 900);
    assert!(display.performance.memory_enabled);
    assert_eq!(display.performance.memory_sample_interval_secs, 0);
    assert_eq!(display.performance.memory_delta_threshold_bytes, 0);
    assert!(display.performance.heap_profile_enabled);
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
    per_source.insert(
        SettingSource::Project,
        raw_syntax_highlighting(SyntaxHighlightingLevel::Off),
    );
    per_source.insert(
        SettingSource::Local,
        raw_syntax_highlighting(SyntaxHighlightingLevel::Full),
    );
    let settings = settings_with_source(Settings::default(), per_source);

    let display = DisplaySettings::from_settings_with_sources(&settings);

    // Merged is `Settings::default()`, whose tier default is `Lite`.
    assert_eq!(display.syntax_highlighting, SyntaxHighlighting::Lite);
    assert_eq!(
        display.syntax_highlighting_editability,
        DisplaySettingEditability::OverriddenBy(SettingSource::Local)
    );
}
