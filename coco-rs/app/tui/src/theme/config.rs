use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use ratatui::style::Color;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;

use super::Theme;
use super::ThemeName;

const CONFIG_FILE_NAME: &str = "theme.json";

#[derive(Debug, Clone)]
pub struct ThemeLoadResult {
    pub state: ThemeRuntimeState,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ThemeRuntimeState {
    pub config_path: PathBuf,
    pub setting: ThemeSetting,
    pub active_id: String,
    pub theme: Theme,
    pub registry: ThemeRegistry,
    pub choices: Vec<ThemeChoice>,
}

impl ThemeRuntimeState {
    pub fn load_default_path() -> Result<Self> {
        let path = theme_config_path();
        if path.exists() {
            return Self::load_from_path(path);
        }
        // No TUI-local `theme.json`: honor `GlobalConfig.theme` (`~/.coco.json`).
        let config = ThemeConfig {
            active: global_theme_setting().unwrap_or_default(),
            ..ThemeConfig::default()
        };
        Self::from_config(path, config)
    }

    pub fn load_from_path(path: PathBuf) -> Result<Self> {
        let config = load_theme_config(&path)?;
        Self::from_config(path, config)
    }

    pub fn from_config(path: PathBuf, config: ThemeConfig) -> Result<Self> {
        if config.version != 1 {
            bail!("unsupported theme config version {}", config.version);
        }
        let registry = ThemeRegistry::from_config(&config)?;
        let resolved = registry.resolve_setting(&config.active)?;
        let setting = canonical_setting(&config.active, &resolved.id);
        let choices = registry.choices();
        Ok(Self {
            config_path: path,
            setting,
            active_id: resolved.id,
            theme: resolved.theme,
            registry,
            choices,
        })
    }

    pub fn with_setting(&self, setting: ThemeSetting) -> Result<Self> {
        let resolved = self.registry.resolve_setting(&setting)?;
        let setting = canonical_setting(&setting, &resolved.id);
        let mut next = self.clone();
        next.setting = setting;
        next.active_id = resolved.id;
        next.theme = resolved.theme;
        Ok(next)
    }
}

impl Default for ThemeRuntimeState {
    fn default() -> Self {
        let registry = ThemeRegistry::default();
        let setting = ThemeSetting::default();
        Self {
            config_path: theme_config_path(),
            setting,
            active_id: ThemeName::Dark.id().to_string(),
            theme: Theme::from_name(ThemeName::Dark),
            choices: registry.choices(),
            registry,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeChoice {
    pub setting: ThemeSetting,
    pub id: String,
    pub label: String,
}

impl ThemeChoice {
    fn auto() -> Self {
        Self {
            setting: ThemeSetting::Auto,
            id: "auto".to_string(),
            label: "Auto (match terminal)".to_string(),
        }
    }

    fn builtin(name: ThemeName) -> Self {
        let id = name.id().to_string();
        Self {
            setting: ThemeSetting::Named(id.clone()),
            id,
            label: name.label().to_string(),
        }
    }

    fn custom(id: &str) -> Self {
        Self {
            setting: ThemeSetting::Named(id.to_string()),
            id: id.to_string(),
            label: id.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeSetting {
    Auto,
    Named(String),
}

impl ThemeSetting {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Auto => "auto",
            Self::Named(id) => id.as_str(),
        }
    }
}

impl Default for ThemeSetting {
    fn default() -> Self {
        Self::Named(ThemeName::Dark.id().to_string())
    }
}

impl Serialize for ThemeSetting {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ThemeSetting {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        let trimmed = raw.trim();
        if trimmed.eq_ignore_ascii_case("auto") {
            Ok(Self::Auto)
        } else {
            Ok(Self::Named(trimmed.to_string()))
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeConfig {
    #[serde(rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub version: i32,
    pub active: ThemeSetting,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub themes: BTreeMap<String, ThemeDefinition>,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            schema: None,
            version: 1,
            active: ThemeSetting::default(),
            themes: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeDefinition {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<ThemeMode>,
    #[serde(skip_serializing_if = "PartialThemeColors::is_empty")]
    pub colors: PartialThemeColors,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    Dark,
    Light,
    Ansi,
}

#[derive(Debug, Clone, Default)]
pub struct ThemeRegistry {
    custom: BTreeMap<String, ThemeDefinition>,
}

#[derive(Debug, Clone)]
struct ResolvedTheme {
    id: String,
    theme: Theme,
}

impl ThemeRegistry {
    pub fn from_config(config: &ThemeConfig) -> Result<Self> {
        for id in config.themes.keys() {
            if id == "auto" {
                bail!("custom theme id `auto` is reserved");
            }
            if ThemeName::from_id(id).is_some() {
                bail!("custom theme `{id}` cannot shadow a built-in theme");
            }
        }
        Ok(Self {
            custom: config.themes.clone(),
        })
    }

    pub fn choices(&self) -> Vec<ThemeChoice> {
        let mut choices = Vec::with_capacity(1 + ThemeName::all().len() + self.custom.len());
        choices.push(ThemeChoice::auto());
        choices.extend(ThemeName::all().iter().copied().map(ThemeChoice::builtin));
        choices.extend(self.custom.keys().map(|id| ThemeChoice::custom(id)));
        choices
    }

    fn resolve_setting(&self, setting: &ThemeSetting) -> Result<ResolvedTheme> {
        let id = match setting {
            ThemeSetting::Auto => system_theme_id(),
            ThemeSetting::Named(id) => id.as_str(),
        };
        self.resolve_id(id, &mut Vec::new())
    }

    fn resolve_id(&self, id: &str, stack: &mut Vec<String>) -> Result<ResolvedTheme> {
        if let Some(name) = ThemeName::from_id(id) {
            return Ok(ResolvedTheme {
                id: name.id().to_string(),
                theme: Theme::from_name(name),
            });
        }

        if stack.iter().any(|seen| seen == id) {
            let mut cycle = stack.join(" -> ");
            if !cycle.is_empty() {
                cycle.push_str(" -> ");
            }
            cycle.push_str(id);
            bail!("theme inheritance cycle: {cycle}");
        }

        let definition = self
            .custom
            .get(id)
            .with_context(|| format!("unknown theme `{id}`"))?;
        stack.push(id.to_string());
        let base_id = definition
            .extends
            .as_deref()
            .unwrap_or_else(|| match definition.mode {
                Some(ThemeMode::Light) => ThemeName::Light.id(),
                Some(ThemeMode::Dark) => ThemeName::Dark.id(),
                Some(ThemeMode::Ansi) => ThemeName::DarkAnsi.id(),
                None => ThemeName::Dark.id(),
            });
        let mut resolved = self.resolve_id(base_id, stack)?;
        stack.pop();
        apply_colors(&mut resolved.theme, &definition.colors)
            .with_context(|| format!("failed to apply colors for theme `{id}`"))?;
        resolved.id = id.to_string();
        Ok(resolved)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PartialThemeColors {
    pub primary: Option<String>,
    pub secondary: Option<String>,
    pub accent: Option<String>,
    pub text: Option<String>,
    pub text_dim: Option<String>,
    pub text_bold: Option<String>,
    pub user_message: Option<String>,
    pub user_message_bg: Option<String>,
    pub assistant_message: Option<String>,
    pub thinking: Option<String>,
    pub system_message: Option<String>,
    pub tool_running: Option<String>,
    pub tool_completed: Option<String>,
    pub tool_error: Option<String>,
    pub warning: Option<String>,
    pub success: Option<String>,
    pub error: Option<String>,
    pub border: Option<String>,
    pub border_focused: Option<String>,
    pub modal_border: Option<String>,
    pub panel_border: Option<String>,
    pub scrollbar: Option<String>,
    pub plan_mode: Option<String>,
    pub selection_bg: Option<String>,
    pub selection_fg: Option<String>,
    pub diff_added: Option<String>,
    pub diff_removed: Option<String>,
    pub code_keyword: Option<String>,
    pub code_string: Option<String>,
    pub code_comment: Option<String>,
    pub code_number: Option<String>,
    pub code_function: Option<String>,
    pub code_type: Option<String>,
    pub code_operator: Option<String>,
    pub code_inline: Option<String>,
    pub code_bg: Option<String>,
    pub blockquote: Option<String>,
    pub heading: Option<String>,
    pub hr: Option<String>,
    pub strikethrough: Option<String>,
    pub hyperlink: Option<String>,
    pub table_border: Option<String>,
    pub table_header: Option<String>,
    pub search_match: Option<String>,
    pub progress_bar: Option<String>,
    pub context_used: Option<String>,
    pub context_free: Option<String>,
}

impl PartialThemeColors {
    fn is_empty(&self) -> bool {
        self.primary.is_none()
            && self.secondary.is_none()
            && self.accent.is_none()
            && self.text.is_none()
            && self.text_dim.is_none()
            && self.text_bold.is_none()
            && self.user_message.is_none()
            && self.user_message_bg.is_none()
            && self.assistant_message.is_none()
            && self.thinking.is_none()
            && self.system_message.is_none()
            && self.tool_running.is_none()
            && self.tool_completed.is_none()
            && self.tool_error.is_none()
            && self.warning.is_none()
            && self.success.is_none()
            && self.error.is_none()
            && self.border.is_none()
            && self.border_focused.is_none()
            && self.modal_border.is_none()
            && self.panel_border.is_none()
            && self.scrollbar.is_none()
            && self.plan_mode.is_none()
            && self.selection_bg.is_none()
            && self.selection_fg.is_none()
            && self.diff_added.is_none()
            && self.diff_removed.is_none()
            && self.code_keyword.is_none()
            && self.code_string.is_none()
            && self.code_comment.is_none()
            && self.code_number.is_none()
            && self.code_function.is_none()
            && self.code_type.is_none()
            && self.code_operator.is_none()
            && self.code_inline.is_none()
            && self.code_bg.is_none()
            && self.blockquote.is_none()
            && self.heading.is_none()
            && self.hr.is_none()
            && self.strikethrough.is_none()
            && self.hyperlink.is_none()
            && self.table_border.is_none()
            && self.table_header.is_none()
            && self.search_match.is_none()
            && self.progress_bar.is_none()
            && self.context_used.is_none()
            && self.context_free.is_none()
    }
}

pub fn theme_config_path() -> PathBuf {
    coco_config::global_config::config_home().join(CONFIG_FILE_NAME)
}

pub fn load_theme_runtime_or_default() -> ThemeLoadResult {
    // Palette downsampling to the terminal's color depth happens at the single
    // install chokepoint `UiState::apply_theme_runtime` (which also covers
    // in-app theme switches), so it is intentionally NOT applied here.
    match ThemeRuntimeState::load_default_path() {
        Ok(state) => ThemeLoadResult { state, error: None },
        Err(err) => ThemeLoadResult {
            state: ThemeRuntimeState::default(),
            error: Some(format!(
                "failed to load {}: {err}",
                theme_config_path().display()
            )),
        },
    }
}

pub fn save_theme_setting(setting: &ThemeSetting) -> Result<PathBuf> {
    let path = theme_config_path();
    save_theme_setting_to_path(&path, setting)?;
    Ok(path)
}

pub(crate) fn save_theme_setting_to_path(path: &Path, setting: &ThemeSetting) -> Result<()> {
    let mut config = match load_theme_config(path) {
        Ok(config) => config,
        Err(err) if path.exists() && std::fs::read_to_string(path).is_ok() => {
            let backup = backup_invalid_theme_config(path)?;
            tracing::warn!(
                path = %path.display(),
                backup = %backup.display(),
                error = %err,
                "replacing invalid theme config while saving theme setting"
            );
            ThemeConfig::default()
        }
        Err(err) => return Err(err),
    };
    config.version = 1;
    config.active = setting.clone();
    write_theme_config(path, &config)
}

fn canonical_setting(setting: &ThemeSetting, resolved_id: &str) -> ThemeSetting {
    match setting {
        ThemeSetting::Auto => ThemeSetting::Auto,
        ThemeSetting::Named(_) => ThemeSetting::Named(resolved_id.to_string()),
    }
}

fn load_theme_config(path: &Path) -> Result<ThemeConfig> {
    if !path.exists() {
        return Ok(ThemeConfig::default());
    }
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    if contents.trim().is_empty() {
        return Ok(ThemeConfig::default());
    }
    serde_json::from_str(&contents).with_context(|| format!("invalid {}", path.display()))
}

fn write_theme_config(path: &Path, config: &ThemeConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let contents = serde_json::to_string_pretty(config)?;
    let tmp = unique_sidecar_path(path, "tmp");
    std::fs::write(&tmp, contents).with_context(|| format!("failed to write {}", tmp.display()))?;
    if let Err(err) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err).with_context(|| {
            format!(
                "failed to replace {} with {}",
                path.display(),
                tmp.display()
            )
        });
    }
    Ok(())
}

fn backup_invalid_theme_config(path: &Path) -> Result<PathBuf> {
    let backup = unique_sidecar_path(path, "invalid.bak");
    std::fs::copy(path, &backup).with_context(|| {
        format!(
            "failed to back up invalid theme config {} to {}",
            path.display(),
            backup.display()
        )
    })?;
    Ok(backup)
}

fn unique_sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(CONFIG_FILE_NAME);
    path.with_file_name(format!(".{file_name}.{}.{}", uuid::Uuid::new_v4(), suffix))
}

fn apply_colors(theme: &mut Theme, colors: &PartialThemeColors) -> Result<()> {
    apply_color(&mut theme.primary, "primary", &colors.primary)?;
    apply_color(&mut theme.secondary, "secondary", &colors.secondary)?;
    apply_color(&mut theme.accent, "accent", &colors.accent)?;
    apply_color(&mut theme.text, "text", &colors.text)?;
    apply_color(&mut theme.text_dim, "text_dim", &colors.text_dim)?;
    apply_color(&mut theme.text_bold, "text_bold", &colors.text_bold)?;
    apply_color(
        &mut theme.user_message,
        "user_message",
        &colors.user_message,
    )?;
    apply_optional_color(
        &mut theme.user_message_bg,
        "user_message_bg",
        &colors.user_message_bg,
    )?;
    apply_color(
        &mut theme.assistant_message,
        "assistant_message",
        &colors.assistant_message,
    )?;
    apply_color(&mut theme.thinking, "thinking", &colors.thinking)?;
    apply_color(
        &mut theme.system_message,
        "system_message",
        &colors.system_message,
    )?;
    apply_color(
        &mut theme.tool_running,
        "tool_running",
        &colors.tool_running,
    )?;
    apply_color(
        &mut theme.tool_completed,
        "tool_completed",
        &colors.tool_completed,
    )?;
    apply_color(&mut theme.tool_error, "tool_error", &colors.tool_error)?;
    apply_color(&mut theme.warning, "warning", &colors.warning)?;
    apply_color(&mut theme.success, "success", &colors.success)?;
    apply_color(&mut theme.error, "error", &colors.error)?;
    apply_color(&mut theme.border, "border", &colors.border)?;
    apply_color(
        &mut theme.border_focused,
        "border_focused",
        &colors.border_focused,
    )?;
    apply_color(
        &mut theme.modal_border,
        "modal_border",
        &colors.modal_border,
    )?;
    apply_color(
        &mut theme.panel_border,
        "panel_border",
        &colors.panel_border,
    )?;
    apply_color(&mut theme.scrollbar, "scrollbar", &colors.scrollbar)?;
    apply_color(&mut theme.plan_mode, "plan_mode", &colors.plan_mode)?;
    apply_color(
        &mut theme.selection_bg,
        "selection_bg",
        &colors.selection_bg,
    )?;
    apply_color(
        &mut theme.selection_fg,
        "selection_fg",
        &colors.selection_fg,
    )?;
    apply_color(&mut theme.diff_added, "diff_added", &colors.diff_added)?;
    apply_color(
        &mut theme.diff_removed,
        "diff_removed",
        &colors.diff_removed,
    )?;
    apply_color(
        &mut theme.code_keyword,
        "code_keyword",
        &colors.code_keyword,
    )?;
    apply_color(&mut theme.code_string, "code_string", &colors.code_string)?;
    apply_color(
        &mut theme.code_comment,
        "code_comment",
        &colors.code_comment,
    )?;
    apply_color(&mut theme.code_number, "code_number", &colors.code_number)?;
    apply_color(
        &mut theme.code_function,
        "code_function",
        &colors.code_function,
    )?;
    apply_color(&mut theme.code_type, "code_type", &colors.code_type)?;
    apply_color(
        &mut theme.code_operator,
        "code_operator",
        &colors.code_operator,
    )?;
    apply_color(&mut theme.code_inline, "code_inline", &colors.code_inline)?;
    apply_optional_color(&mut theme.code_bg, "code_bg", &colors.code_bg)?;
    apply_color(&mut theme.blockquote, "blockquote", &colors.blockquote)?;
    apply_color(&mut theme.heading, "heading", &colors.heading)?;
    apply_color(&mut theme.hr, "hr", &colors.hr)?;
    apply_color(
        &mut theme.strikethrough,
        "strikethrough",
        &colors.strikethrough,
    )?;
    apply_color(&mut theme.hyperlink, "hyperlink", &colors.hyperlink)?;
    apply_color(
        &mut theme.table_border,
        "table_border",
        &colors.table_border,
    )?;
    apply_color(
        &mut theme.table_header,
        "table_header",
        &colors.table_header,
    )?;
    apply_color(
        &mut theme.search_match,
        "search_match",
        &colors.search_match,
    )?;
    apply_color(
        &mut theme.progress_bar,
        "progress_bar",
        &colors.progress_bar,
    )?;
    apply_color(
        &mut theme.context_used,
        "context_used",
        &colors.context_used,
    )?;
    apply_color(
        &mut theme.context_free,
        "context_free",
        &colors.context_free,
    )?;
    Ok(())
}

fn apply_color(slot: &mut Color, name: &str, value: &Option<String>) -> Result<()> {
    if let Some(raw) = value {
        *slot = parse_theme_color(raw).with_context(|| format!("invalid color `{name}`"))?;
    }
    Ok(())
}

fn apply_optional_color(
    slot: &mut Option<Color>,
    name: &str,
    value: &Option<String>,
) -> Result<()> {
    if let Some(raw) = value {
        *slot =
            parse_optional_theme_color(raw).with_context(|| format!("invalid color `{name}`"))?;
    }
    Ok(())
}

fn parse_optional_theme_color(raw: &str) -> Result<Option<Color>> {
    let trimmed = raw.trim();
    if matches!(trimmed.to_ascii_lowercase().as_str(), "none" | "null") {
        return Ok(None);
    }
    parse_theme_color(trimmed).map(Some)
}

#[allow(clippy::disallowed_methods)]
fn parse_theme_color(raw: &str) -> Result<Color> {
    let trimmed = raw.trim();
    let lower = trimmed.to_ascii_lowercase();
    match lower.as_str() {
        "reset" | "default" => return Ok(Color::Reset),
        "black" | "ansi:black" => return Ok(Color::Black),
        "red" | "ansi:red" => return Ok(Color::Red),
        "green" | "ansi:green" => return Ok(Color::Green),
        "yellow" | "ansi:yellow" => return Ok(Color::Yellow),
        "blue" | "ansi:blue" => return Ok(Color::Blue),
        "magenta" | "ansi:magenta" => return Ok(Color::Magenta),
        "cyan" | "ansi:cyan" => return Ok(Color::Cyan),
        "gray" | "grey" | "ansi:gray" | "ansi:grey" => return Ok(Color::Gray),
        "dark_gray" | "dark-grey" | "dark-gray" | "blackbright" | "black_bright"
        | "black-bright" | "ansi:dark_gray" | "ansi:blackbright" | "ansi:black_bright"
        | "ansi:black-bright" => {
            return Ok(Color::DarkGray);
        }
        "light_red" | "light-red" | "redbright" | "red_bright" | "red-bright"
        | "ansi:light_red" | "ansi:redbright" | "ansi:red_bright" | "ansi:red-bright" => {
            return Ok(Color::LightRed);
        }
        "light_green" | "light-green" | "greenbright" | "green_bright" | "green-bright"
        | "ansi:light_green" | "ansi:greenbright" | "ansi:green_bright" | "ansi:green-bright" => {
            return Ok(Color::LightGreen);
        }
        "light_yellow" | "light-yellow" | "yellowbright" | "yellow_bright" | "yellow-bright"
        | "ansi:light_yellow" | "ansi:yellowbright" | "ansi:yellow_bright"
        | "ansi:yellow-bright" => return Ok(Color::LightYellow),
        "light_blue" | "light-blue" | "bluebright" | "blue_bright" | "blue-bright"
        | "ansi:light_blue" | "ansi:bluebright" | "ansi:blue_bright" | "ansi:blue-bright" => {
            return Ok(Color::LightBlue);
        }
        "light_magenta"
        | "light-magenta"
        | "magentabright"
        | "magenta_bright"
        | "magenta-bright"
        | "ansi:light_magenta"
        | "ansi:magentabright"
        | "ansi:magenta_bright"
        | "ansi:magenta-bright" => {
            return Ok(Color::LightMagenta);
        }
        "light_cyan" | "light-cyan" | "cyanbright" | "cyan_bright" | "cyan-bright"
        | "ansi:light_cyan" | "ansi:cyanbright" | "ansi:cyan_bright" | "ansi:cyan-bright" => {
            return Ok(Color::LightCyan);
        }
        "white" | "ansi:white" | "whitebright" | "white_bright" | "white-bright"
        | "ansi:whitebright" | "ansi:white_bright" | "ansi:white-bright" => {
            return Ok(Color::White);
        }
        _ => {}
    }

    if let Some(hex) = lower.strip_prefix('#') {
        return parse_hex_color(hex);
    }
    if let Some(inner) = lower
        .strip_prefix("rgb(")
        .and_then(|value| value.strip_suffix(')'))
    {
        return parse_rgb_color(inner);
    }
    if let Some(inner) = lower
        .strip_prefix("ansi256(")
        .and_then(|value| value.strip_suffix(')'))
    {
        let index = inner.trim().parse::<u8>()?;
        return Ok(Color::Indexed(index));
    }
    if let Some(index) = lower.strip_prefix("ansi256:") {
        let index = index.trim().parse::<u8>()?;
        return Ok(Color::Indexed(index));
    }

    bail!("unsupported color `{raw}`")
}

#[allow(clippy::disallowed_methods)]
fn parse_hex_color(hex: &str) -> Result<Color> {
    match hex.len() {
        3 => {
            let r = expand_hex_digit(&hex[0..1])?;
            let g = expand_hex_digit(&hex[1..2])?;
            let b = expand_hex_digit(&hex[2..3])?;
            Ok(Color::Rgb(r, g, b))
        }
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16)?;
            let g = u8::from_str_radix(&hex[2..4], 16)?;
            let b = u8::from_str_radix(&hex[4..6], 16)?;
            Ok(Color::Rgb(r, g, b))
        }
        _ => bail!("hex colors must use #rgb or #rrggbb"),
    }
}

fn expand_hex_digit(value: &str) -> Result<u8> {
    let n = u8::from_str_radix(value, 16)?;
    Ok(n * 17)
}

#[allow(clippy::disallowed_methods)]
fn parse_rgb_color(inner: &str) -> Result<Color> {
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    if parts.len() != 3 {
        bail!("rgb() colors must have three components");
    }
    Ok(Color::Rgb(
        parts[0].parse::<u8>()?,
        parts[1].parse::<u8>()?,
        parts[2].parse::<u8>()?,
    ))
}

fn system_theme_id() -> &'static str {
    use coco_tui_ui::system_theme::SystemTheme;

    // Prefer an OSC 11 background probe (terminal's actual background); fall
    // back to the synchronous `$COLORFGBG` seed, then dark. The probe (when the
    // active setting is `auto`) runs once at TUI boot via `install_theme`.
    let detected = coco_tui_ui::system_theme::cached_system_theme().or_else(|| {
        std::env::var("COLORFGBG")
            .ok()
            .and_then(|value| coco_tui_ui::system_theme::detect_from_colorfgbg(&value))
    });
    match detected.unwrap_or(SystemTheme::Dark) {
        SystemTheme::Light => ThemeName::Light.id(),
        SystemTheme::Dark => ThemeName::Dark.id(),
    }
}

/// The `active` setting persisted in `theme.json` (without resolving it). Lets
/// the TUI boot decide whether to run the OSC 11 background probe — only `auto`
/// needs it. Defaults to the built-in default on any read/parse failure.
pub(crate) fn persisted_active_setting() -> ThemeSetting {
    let path = theme_config_path();
    if path.exists() {
        return load_theme_config(&path)
            .map(|config| config.active)
            .unwrap_or_default();
    }
    global_theme_setting().unwrap_or_default()
}

/// The theme selection from `GlobalConfig` (`~/.coco.json`). `None` when
/// unset/empty. Consulted only when no TUI-local `theme.json` exists; the
/// in-app picker still writes `theme.json`, which takes precedence.
fn global_theme_setting() -> Option<ThemeSetting> {
    let global = coco_config::global_config::load_global_config().ok()?;
    theme_setting_from_global(global.theme.as_deref()?)
}

/// Map a `GlobalConfig.theme` string to a [`ThemeSetting`] (`"auto"` →
/// [`ThemeSetting::Auto`], otherwise a named theme). `None` for empty input.
pub(crate) fn theme_setting_from_global(theme: &str) -> Option<ThemeSetting> {
    let theme = theme.trim();
    if theme.is_empty() {
        return None;
    }
    Some(if theme.eq_ignore_ascii_case("auto") {
        ThemeSetting::Auto
    } else {
        ThemeSetting::Named(theme.to_string())
    })
}
