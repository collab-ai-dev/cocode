use std::path::PathBuf;

use pretty_assertions::assert_eq;
use ratatui::style::Color;

use super::ThemeConfig;
use super::ThemeRuntimeState;
use super::ThemeSetting;
use super::config::save_theme_setting_to_path;

#[test]
#[allow(clippy::disallowed_methods)]
fn test_theme_runtime_resolves_custom_theme_extending_builtin() -> anyhow::Result<()> {
    let config: ThemeConfig = serde_json::from_str(
        r##"{
          "version": 1,
          "active": "my_dark",
	          "themes": {
	            "my_dark": {
	              "extends": "dark",
	              "colors": {
	                "primary": "#010203",
	                "secondary": "ansi:blackBright",
	                "accent": "ansi:redBright",
	                "selection_bg": "rgb(4,5,6)",
	                "tool_running": "ansi256(208)"
	              }
	            }
	          }
	        }"##,
    )?;

    let state = ThemeRuntimeState::from_config(PathBuf::from("theme.json"), config)?;

    assert_eq!(state.active_id, "my_dark");
    assert_eq!(state.theme.primary, Color::Rgb(1, 2, 3));
    assert_eq!(state.theme.secondary, Color::DarkGray);
    assert_eq!(state.theme.accent, Color::LightRed);
    assert_eq!(state.theme.selection_bg, Color::Rgb(4, 5, 6));
    assert_eq!(state.theme.tool_running, Color::Indexed(208));
    assert!(
        state
            .choices
            .iter()
            .any(|choice| choice.id == "my_dark" && choice.label == "my_dark")
    );
    Ok(())
}

#[test]
fn test_theme_runtime_canonicalizes_builtin_aliases() -> anyhow::Result<()> {
    let config: ThemeConfig = serde_json::from_str(
        r#"{
          "version": 1,
          "active": "dark-ansi"
        }"#,
    )?;

    let state = ThemeRuntimeState::from_config(PathBuf::from("theme.json"), config)?;

    assert_eq!(state.active_id, "dark_ansi");
    assert_eq!(state.setting, ThemeSetting::Named("dark_ansi".to_string()));
    Ok(())
}

#[test]
#[allow(clippy::disallowed_methods)]
fn test_theme_definition_mode_selects_base_without_extends() -> anyhow::Result<()> {
    let config: ThemeConfig = serde_json::from_str(
        r#"{
          "version": 1,
          "active": "my_light",
          "themes": {
            "my_light": {
              "mode": "light"
            }
          }
        }"#,
    )?;

    let state = ThemeRuntimeState::from_config(PathBuf::from("theme.json"), config)?;

    assert_eq!(state.active_id, "my_light");
    assert_eq!(state.theme.selection_bg, Color::Rgb(180, 213, 255));
    Ok(())
}

#[test]
fn test_theme_runtime_rejects_invalid_custom_color() -> anyhow::Result<()> {
    let config: ThemeConfig = serde_json::from_str(
        r#"{
          "version": 1,
          "active": "bad",
          "themes": {
            "bad": {
              "extends": "dark",
              "colors": {
                "primary": "not-a-color"
              }
            }
          }
        }"#,
    )?;

    let result = ThemeRuntimeState::from_config(PathBuf::from("theme.json"), config);

    assert!(result.is_err());
    Ok(())
}

#[test]
fn test_theme_runtime_ignores_invalid_inactive_custom_theme() -> anyhow::Result<()> {
    let config: ThemeConfig = serde_json::from_str(
        r#"{
          "version": 1,
          "active": "dark",
          "themes": {
            "bad": {
              "extends": "dark",
              "colors": {
                "primary": "not-a-color"
              }
            }
          }
        }"#,
    )?;

    let state = ThemeRuntimeState::from_config(PathBuf::from("theme.json"), config)?;

    assert_eq!(state.active_id, "dark");
    assert!(
        state
            .choices
            .iter()
            .any(|choice| choice.id == "bad" && choice.label == "bad")
    );
    assert!(
        state
            .with_setting(ThemeSetting::Named("bad".to_string()))
            .is_err()
    );
    Ok(())
}

#[test]
fn test_theme_runtime_rejects_unsupported_config_version() -> anyhow::Result<()> {
    let config: ThemeConfig = serde_json::from_str(
        r#"{
          "version": 2,
          "active": "dark"
        }"#,
    )?;

    let result = ThemeRuntimeState::from_config(PathBuf::from("theme.json"), config);

    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("unsupported theme config version 2")
    );
    Ok(())
}

#[test]
fn test_save_theme_setting_replaces_invalid_config_with_backup() -> anyhow::Result<()> {
    let temp_dir = TempThemeDir::new()?;
    let path = temp_dir.path.join("theme.json");
    std::fs::write(&path, "{ invalid json")?;

    save_theme_setting_to_path(&path, &ThemeSetting::Named("light".to_string()))?;

    let saved: ThemeConfig = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    assert_eq!(saved.version, 1);
    assert_eq!(saved.active, ThemeSetting::Named("light".to_string()));

    let backups = std::fs::read_dir(&temp_dir.path)?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .ends_with(".invalid.bak")
        })
        .count();
    assert_eq!(backups, 1);
    Ok(())
}

#[test]
fn test_global_theme_setting_maps_auto_and_named() {
    use super::ThemeSetting;
    use super::config::theme_setting_from_global;

    // `GlobalConfig.theme` ("auto" / a named theme) maps into the TUI's
    // ThemeSetting so `~/.coco.json` drives the theme when no `theme.json`.
    assert_eq!(theme_setting_from_global("auto"), Some(ThemeSetting::Auto));
    assert_eq!(
        theme_setting_from_global("  AUTO "),
        Some(ThemeSetting::Auto)
    );
    assert_eq!(
        theme_setting_from_global("dark"),
        Some(ThemeSetting::Named("dark".to_string()))
    );
    assert_eq!(theme_setting_from_global(""), None);
    assert_eq!(theme_setting_from_global("   "), None);
}

#[test]
fn test_theme_setting_serializes_as_plain_string() -> anyhow::Result<()> {
    let json = serde_json::to_string(&ThemeSetting::Auto)?;
    assert_eq!(json, r#""auto""#);

    let setting: ThemeSetting = serde_json::from_str(r#""dark_ansi""#)?;
    assert_eq!(setting, ThemeSetting::Named("dark_ansi".to_string()));
    Ok(())
}

struct TempThemeDir {
    path: PathBuf,
}

impl TempThemeDir {
    fn new() -> anyhow::Result<Self> {
        let path = std::env::temp_dir().join(format!("coco-theme-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path)?;
        Ok(Self { path })
    }
}

impl Drop for TempThemeDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
