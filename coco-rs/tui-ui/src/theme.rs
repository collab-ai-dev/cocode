//! Theme system for TUI colors.
//!
//! Each theme provides a full color palette. Widgets reference theme
//! fields instead of hardcoding colors, enabling runtime theme switching.

use ratatui::style::Color;

use crate::color::ColorCapability;
use crate::color::adapt_color;

/// Available theme names. Mirrors claude-code's `THEME_NAMES`
/// (`utils/theme.ts`) one-for-one — no coco-only extras. `Dark` is the
/// default (TS `config.ts` → `theme: 'dark'`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeName {
    #[default]
    Dark,
    Light,
    DarkDaltonized,
    LightDaltonized,
    DarkAnsi,
    LightAnsi,
}

impl ThemeName {
    pub fn all() -> &'static [Self] {
        &[
            Self::Dark,
            Self::Light,
            Self::DarkDaltonized,
            Self::LightDaltonized,
            Self::DarkAnsi,
            Self::LightAnsi,
        ]
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
            Self::DarkDaltonized => "dark_daltonized",
            Self::LightDaltonized => "light_daltonized",
            Self::DarkAnsi => "dark_ansi",
            Self::LightAnsi => "light_ansi",
        }
    }

    pub fn label(self) -> &'static str {
        // Friendly names mirroring claude-code's ThemePicker options verbatim.
        match self {
            Self::Dark => "Dark mode",
            Self::Light => "Light mode",
            Self::DarkDaltonized => "Dark mode (colorblind-friendly)",
            Self::LightDaltonized => "Light mode (colorblind-friendly)",
            Self::DarkAnsi => "Dark mode (ANSI colors only)",
            Self::LightAnsi => "Light mode (ANSI colors only)",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            "dark_daltonized" | "dark-daltonized" => Some(Self::DarkDaltonized),
            "light_daltonized" | "light-daltonized" => Some(Self::LightDaltonized),
            "dark_ansi" | "dark-ansi" => Some(Self::DarkAnsi),
            "light_ansi" | "light-ansi" => Some(Self::LightAnsi),
            _ => None,
        }
    }
}

/// Complete color palette for TUI rendering.
///
/// Color discipline:
/// - Avoid Blue for text (hard to read on dark terminals)
/// - Avoid Yellow for backgrounds (invisible on light terminals)
/// - Never use `.white()` — prefer Reset (inherits terminal foreground)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Theme {
    // ── Base ──
    pub primary: Color,
    pub secondary: Color,
    pub accent: Color,

    // ── Text ──
    pub text: Color,
    pub text_dim: Color,
    pub text_bold: Color,

    // ── Messages ──
    pub user_message: Color,
    /// Optional background tint applied to user-message lines.
    /// A subtle tint so user prompts separate visually from assistant prose.
    /// None = inherit terminal background.
    pub user_message_bg: Option<Color>,
    pub assistant_message: Color,
    pub thinking: Color,
    pub system_message: Color,

    // ── Status ──
    pub tool_running: Color,
    pub tool_completed: Color,
    pub tool_error: Color,
    pub warning: Color,
    pub success: Color,
    pub error: Color,

    // ── UI Elements ──
    pub border: Color,
    pub border_focused: Color,
    /// Border color for modal/overlay surfaces. Lets modals theme their frame
    /// independently of the generic `border`.
    pub modal_border: Color,
    /// Border color for side/info panels.
    pub panel_border: Color,
    pub scrollbar: Color,
    pub plan_mode: Color,
    pub selection_bg: Color,
    pub selection_fg: Color,

    // ── Diff ──
    pub diff_added: Color,
    pub diff_removed: Color,

    // ── Code highlighting ──
    pub code_keyword: Color,
    pub code_string: Color,
    pub code_comment: Color,
    pub code_number: Color,
    /// Function / method name highlight (syntect `entity.name.function.*`).
    pub code_function: Color,
    /// Type / class name highlight (syntect `storage.type.*`, `entity.name.type.*`).
    pub code_type: Color,
    /// Operator / punctuation highlight (syntect `keyword.operator.*`).
    pub code_operator: Color,
    /// Inline `code` span foreground (markdown backticks). Decoupled from
    /// `accent` so prose-context inline code can be tuned soft without
    /// recoloring the accent-driven chips / alerts that also read `accent`.
    pub code_inline: Color,
    /// Optional background fill behind fenced code blocks. `None` inherits the
    /// terminal background.
    pub code_bg: Option<Color>,

    // ── Markdown blocks ──
    /// Block-quote gutter / text color (plain quotes; GFM alerts reuse status colors).
    pub blockquote: Color,
    /// ATX heading foreground.
    pub heading: Color,
    /// Horizontal-rule color.
    pub hr: Color,
    /// Struck-through (`~~text~~`) foreground.
    pub strikethrough: Color,

    // ── Extended UI ──
    pub hyperlink: Color,
    pub table_border: Color,
    pub table_header: Color,
    pub search_match: Color,
    pub progress_bar: Color,
    pub context_used: Color,
    pub context_free: Color,
}

impl Theme {
    /// Get theme by name.
    pub fn from_name(name: ThemeName) -> Self {
        match name {
            ThemeName::Dark => Self::dark_theme(),
            ThemeName::Light => Self::light_theme(),
            ThemeName::DarkDaltonized => Self::dark_daltonized_theme(),
            ThemeName::LightDaltonized => Self::light_daltonized_theme(),
            ThemeName::DarkAnsi => Self::dark_ansi_theme(),
            ThemeName::LightAnsi => Self::light_ansi_theme(),
        }
    }

    pub fn builtin(id: &str) -> Option<Self> {
        ThemeName::from_id(id).map(Self::from_name)
    }

    #[allow(clippy::disallowed_methods)]
    fn dark_theme() -> Self {
        // Explicit-RGB palette mapped to Claude Code's TS dark theme — NO
        // ANSI-named colors except `Color::Reset` (body text inherits the
        // terminal foreground). Named colors are recolored by the terminal's own
        // 16-color palette (a custom palette, e.g. magenta→red, would tint the
        // whole UI), so every token is a fixed RGB that also downsamples to a
        // stable xterm-256 index. Token → TS dark: primary/assistant/heading =
        // `claude`, accent = `permission` (periwinkle), grays =
        // `subtle`/`inactive`/`promptBorder`; success/warning/error/planMode/
        // selectionBg are taken verbatim.
        Self {
            modal_border: Color::Rgb(80, 80, 80),
            panel_border: Color::Rgb(80, 80, 80),
            code_function: Color::Rgb(86, 182, 194),
            code_type: Color::Rgb(229, 192, 123),
            code_operator: Color::Rgb(130, 137, 151),
            code_inline: Color::Rgb(177, 185, 249), // = accent/permission (TS codespan)
            code_bg: None,
            blockquote: Color::Rgb(80, 80, 80),
            heading: Color::Rgb(215, 119, 87),
            hr: Color::Rgb(80, 80, 80),
            strikethrough: Color::Rgb(80, 80, 80),
            primary: Color::Rgb(215, 119, 87),
            secondary: Color::Rgb(80, 80, 80),
            // TS `permission`/`suggestion` periwinkle — TS's pervasive cool
            // accent; replaces the palette-recolorable `Magenta`.
            accent: Color::Rgb(177, 185, 249),

            text: Color::Reset,
            text_dim: Color::Rgb(153, 153, 153),
            text_bold: Color::Reset,

            user_message: Color::Rgb(78, 186, 101),
            user_message_bg: Some(Color::Rgb(55, 55, 55)),
            assistant_message: Color::Rgb(215, 119, 87),
            // Thinking renders as normal text (`Color::Reset` = terminal default
            // fg); the renderer de-emphasizes with `.dim().italic()`. Themeable.
            thinking: Color::Reset,
            system_message: Color::Rgb(80, 80, 80),

            tool_running: Color::Rgb(255, 193, 7),
            tool_completed: Color::Rgb(78, 186, 101),
            tool_error: Color::Rgb(255, 107, 128),
            warning: Color::Rgb(255, 193, 7),
            success: Color::Rgb(78, 186, 101),
            error: Color::Rgb(255, 107, 128),

            border: Color::Rgb(136, 136, 136),
            border_focused: Color::Rgb(177, 185, 249),
            scrollbar: Color::Rgb(80, 80, 80),
            plan_mode: Color::Rgb(72, 150, 140),
            selection_bg: Color::Rgb(38, 79, 120),
            selection_fg: Color::Rgb(177, 185, 249),

            diff_added: Color::Rgb(78, 186, 101),
            diff_removed: Color::Rgb(255, 107, 128),

            // One Dark soft code palette (fixed truecolor, keyword BOLD dropped).
            // TS's md fence uses cli-highlight (blue keyword); One Dark's soft
            // purple reads calmer than Monokai's `#F92672` (which quantized to a
            // red `#FF005F` at 256-color).
            code_keyword: Color::Rgb(198, 120, 221),
            code_string: Color::Rgb(152, 195, 121),
            code_comment: Color::Rgb(92, 99, 112),
            code_number: Color::Rgb(209, 154, 102),

            hyperlink: Color::Rgb(122, 180, 232),
            table_border: Color::Rgb(80, 80, 80),
            table_header: Color::Rgb(215, 119, 87),
            search_match: Color::Rgb(255, 193, 7),
            progress_bar: Color::Rgb(177, 185, 249),
            context_used: Color::Rgb(177, 185, 249),
            context_free: Color::Rgb(80, 80, 80),
        }
    }

    #[allow(clippy::disallowed_methods)]
    fn light_theme() -> Self {
        // Explicit-RGB palette mapped to TS `lightTheme` — same discipline as
        // `dark_theme` (no palette-recolorable ANSI-named UI tokens). Token →
        // TS light: primary/assistant/heading/table_header = `claude`, accent/
        // code_inline/focus = `permission` (medium blue), grays = `subtle`/
        // `inactive`/`promptBorder`; success/warning/error/planMode/selectionBg
        // verbatim; hyperlink = `briefLabelYou`. Code-syntax keeps GitHub-light.
        Self {
            modal_border: Color::Rgb(175, 175, 175),
            panel_border: Color::Rgb(175, 175, 175),
            // GitHub-light syntax palette (mirrors claude-code's light file
            // preview, `GITHUB_SCOPES`); keyword BOLD dropped in the renderer.
            code_function: Color::Rgb(121, 93, 163),
            code_type: Color::Rgb(0, 134, 179),
            code_operator: Color::Rgb(150, 152, 150),
            code_inline: Color::Rgb(87, 105, 247), // = permission (TS light codespan)
            code_bg: None,
            blockquote: Color::Rgb(175, 175, 175),
            heading: Color::Rgb(215, 119, 87),
            hr: Color::Rgb(175, 175, 175),
            strikethrough: Color::Rgb(175, 175, 175),
            primary: Color::Rgb(215, 119, 87),
            secondary: Color::Rgb(175, 175, 175),
            accent: Color::Rgb(87, 105, 247),

            text: Color::Reset,
            text_dim: Color::Rgb(102, 102, 102),
            text_bold: Color::Reset,

            user_message: Color::Rgb(44, 122, 57),
            user_message_bg: Some(Color::Rgb(240, 240, 240)),
            assistant_message: Color::Rgb(215, 119, 87),
            thinking: Color::Reset,
            system_message: Color::Rgb(175, 175, 175),

            tool_running: Color::Rgb(150, 108, 30),
            tool_completed: Color::Rgb(44, 122, 57),
            tool_error: Color::Rgb(171, 43, 63),
            warning: Color::Rgb(150, 108, 30),
            success: Color::Rgb(44, 122, 57),
            error: Color::Rgb(171, 43, 63),

            border: Color::Rgb(153, 153, 153),
            border_focused: Color::Rgb(87, 105, 247),
            scrollbar: Color::Rgb(175, 175, 175),
            plan_mode: Color::Rgb(0, 102, 102),
            selection_bg: Color::Rgb(180, 213, 255),
            selection_fg: Color::Rgb(87, 105, 247),

            diff_added: Color::Rgb(44, 122, 57),
            diff_removed: Color::Rgb(171, 43, 63),

            // GitHub-light syntax palette (see light_theme code_function above).
            code_keyword: Color::Rgb(167, 29, 93),
            code_string: Color::Rgb(24, 54, 145),
            code_comment: Color::Rgb(150, 152, 150),
            code_number: Color::Rgb(0, 134, 179),

            hyperlink: Color::Rgb(37, 99, 235),
            table_border: Color::Rgb(175, 175, 175),
            table_header: Color::Rgb(215, 119, 87),
            search_match: Color::Rgb(150, 108, 30),
            progress_bar: Color::Rgb(87, 105, 247),
            context_used: Color::Rgb(87, 105, 247),
            context_free: Color::Rgb(175, 175, 175),
        }
    }

    #[allow(clippy::disallowed_methods)]
    fn dark_daltonized_theme() -> Self {
        // TS `darkDaltonizedTheme`: deuteranopia-adjusted brand/accent + every
        // semantic color, over dark's grays. Code syntax inherits dark's Monokai
        // (TS keeps `MONOKAI_SCOPES` for dark-daltonized).
        let mut theme = Self::dark_theme();
        // claude (orange adjusted for deuteranopia) → brand tokens
        theme.primary = Color::Rgb(255, 153, 51);
        theme.heading = Color::Rgb(255, 153, 51);
        theme.assistant_message = Color::Rgb(255, 153, 51);
        theme.table_header = Color::Rgb(255, 153, 51);
        // permission (light blue) → accent tokens + inline code
        theme.accent = Color::Rgb(153, 204, 255);
        theme.border_focused = Color::Rgb(153, 204, 255);
        theme.selection_fg = Color::Rgb(153, 204, 255);
        theme.progress_bar = Color::Rgb(153, 204, 255);
        theme.context_used = Color::Rgb(153, 204, 255);
        theme.code_inline = Color::Rgb(153, 204, 255);
        theme.plan_mode = Color::Rgb(102, 153, 153);
        // success (blue instead of green) → success / diff-added family
        theme.success = Color::Rgb(51, 153, 255);
        theme.tool_completed = Color::Rgb(51, 153, 255);
        theme.diff_added = Color::Rgb(51, 153, 255);
        theme.user_message = Color::Rgb(51, 153, 255);
        theme.error = Color::Rgb(255, 102, 102);
        theme.tool_error = Color::Rgb(255, 102, 102);
        theme.diff_removed = Color::Rgb(255, 102, 102);
        theme.warning = Color::Rgb(255, 204, 0);
        theme.tool_running = Color::Rgb(255, 204, 0);
        theme.search_match = Color::Rgb(255, 204, 0);
        theme
    }

    #[allow(clippy::disallowed_methods)]
    fn light_daltonized_theme() -> Self {
        // TS `lightDaltonizedTheme`: deuteranopia-adjusted brand/accent + every
        // semantic color, over light's grays + GitHub-light syntax.
        let mut theme = Self::light_theme();
        theme.primary = Color::Rgb(255, 153, 51);
        theme.heading = Color::Rgb(255, 153, 51);
        theme.assistant_message = Color::Rgb(255, 153, 51);
        theme.table_header = Color::Rgb(255, 153, 51);
        // permission (bright blue) → accent tokens + inline code
        theme.accent = Color::Rgb(51, 102, 255);
        theme.border_focused = Color::Rgb(51, 102, 255);
        theme.selection_fg = Color::Rgb(51, 102, 255);
        theme.progress_bar = Color::Rgb(51, 102, 255);
        theme.context_used = Color::Rgb(51, 102, 255);
        theme.code_inline = Color::Rgb(51, 102, 255);
        theme.plan_mode = Color::Rgb(51, 102, 102);
        // success (blue instead of green) → success / diff-added family
        theme.success = Color::Rgb(0, 102, 153);
        theme.tool_completed = Color::Rgb(0, 102, 153);
        theme.diff_added = Color::Rgb(0, 102, 153);
        theme.user_message = Color::Rgb(0, 102, 153);
        theme.error = Color::Rgb(204, 0, 0);
        theme.tool_error = Color::Rgb(204, 0, 0);
        theme.diff_removed = Color::Rgb(204, 0, 0);
        theme.warning = Color::Rgb(255, 153, 0);
        theme.tool_running = Color::Rgb(255, 153, 0);
        theme.search_match = Color::Rgb(255, 153, 0);
        theme.user_message_bg = Some(Color::Rgb(220, 220, 220));
        theme
    }

    fn dark_ansi_theme() -> Self {
        Self {
            modal_border: Color::DarkGray,
            panel_border: Color::DarkGray,
            // ANSI bright-palette syntax (mirrors claude-code's `ANSI_SCOPES`:
            // keyword 13, type 14, fn 11, number 12, string 10, comment 8).
            code_function: Color::LightYellow,
            code_type: Color::LightCyan,
            code_operator: Color::DarkGray,
            code_inline: Color::LightBlue, // = accent/permission (TS codespan, not magenta)
            code_bg: None,
            blockquote: Color::DarkGray,
            // Brand = TS `claude` (ansi:redBright); cool accent = TS `permission`
            // (ansi:blueBright). Chrome grays keep `DarkGray` (ANSI 8) instead of
            // TS's `ansi:white` — a gray dim/border reads better than white.
            heading: Color::LightRed,
            hr: Color::DarkGray,
            strikethrough: Color::DarkGray,
            primary: Color::LightRed,
            secondary: Color::DarkGray,
            accent: Color::LightBlue,

            text: Color::Reset,
            text_dim: Color::DarkGray,
            text_bold: Color::Reset,

            user_message: Color::LightGreen,
            // ANSI-only theme: skip the RGB tint that the truecolor
            // themes use — readers on ANSI-strict terminals get the
            // `❯` prefix as the user-row marker.
            user_message_bg: None,
            assistant_message: Color::LightRed,
            thinking: Color::LightMagenta,
            system_message: Color::DarkGray,

            tool_running: Color::LightYellow,
            tool_completed: Color::LightGreen,
            tool_error: Color::LightRed,
            warning: Color::LightYellow,
            success: Color::LightGreen,
            error: Color::LightRed,

            border: Color::DarkGray,
            border_focused: Color::LightBlue,
            scrollbar: Color::DarkGray,
            plan_mode: Color::LightCyan,
            selection_bg: Color::Blue,
            selection_fg: Color::LightBlue,

            diff_added: Color::LightGreen,
            diff_removed: Color::LightRed,

            code_keyword: Color::LightMagenta,
            code_string: Color::LightGreen,
            code_comment: Color::DarkGray,
            code_number: Color::LightBlue,

            hyperlink: Color::LightBlue,
            table_border: Color::DarkGray,
            table_header: Color::LightRed,
            search_match: Color::LightYellow,
            progress_bar: Color::LightBlue,
            context_used: Color::LightBlue,
            context_free: Color::DarkGray,
        }
    }

    fn light_ansi_theme() -> Self {
        Self {
            modal_border: Color::DarkGray,
            panel_border: Color::DarkGray,
            // ANSI bright-palette syntax — TS uses one `ANSI_SCOPES` map for
            // both ansi themes (`buildTheme` isAnsi branch ignores dark/light).
            code_function: Color::LightYellow,
            code_type: Color::LightCyan,
            code_operator: Color::DarkGray,
            code_inline: Color::Blue, // = accent/permission (TS codespan, not magenta)
            code_bg: None,
            blockquote: Color::DarkGray,
            // Brand = TS `claude` (ansi:redBright); cool accent = TS `permission`
            // (light-ansi uses ansi:blue, not bright). Chrome grays keep
            // `DarkGray` over TS's `ansi:white` for readability.
            heading: Color::LightRed,
            hr: Color::DarkGray,
            strikethrough: Color::DarkGray,
            primary: Color::LightRed,
            secondary: Color::DarkGray,
            accent: Color::Blue,

            text: Color::Reset,
            text_dim: Color::DarkGray,
            text_bold: Color::Reset,

            user_message: Color::Green,
            // ANSI-only theme: skip the RGB tint that the truecolor
            // themes use — readers on ANSI-strict terminals get the
            // `❯` prefix as the user-row marker.
            user_message_bg: None,
            assistant_message: Color::LightRed,
            thinking: Color::Magenta,
            system_message: Color::DarkGray,

            tool_running: Color::Yellow,
            tool_completed: Color::Green,
            tool_error: Color::Red,
            warning: Color::Yellow,
            success: Color::Green,
            error: Color::Red,

            border: Color::DarkGray,
            border_focused: Color::Blue,
            scrollbar: Color::DarkGray,
            plan_mode: Color::Cyan,
            selection_bg: Color::Cyan,
            selection_fg: Color::Blue,

            diff_added: Color::Green,
            diff_removed: Color::Red,

            // ANSI bright-palette syntax (see light_ansi code_function above).
            code_keyword: Color::LightMagenta,
            code_string: Color::LightGreen,
            code_comment: Color::DarkGray,
            code_number: Color::LightBlue,

            hyperlink: Color::Blue,
            table_border: Color::DarkGray,
            table_header: Color::LightRed,
            search_match: Color::Yellow,
            progress_bar: Color::Blue,
            context_used: Color::Blue,
            context_free: Color::DarkGray,
        }
    }
}

impl Theme {
    /// Quantize every palette color to the terminal's color capability.
    ///
    /// Under [`ColorCapability::Ansi256`], `Color::Rgb` fields are mapped to the
    /// nearest xterm-256 palette index ([`adapt_color`]) so a 256-color terminal
    /// gets a deterministic mapping instead of the emulator's (often poorer)
    /// clamp. A no-op under `TrueColor` and for non-RGB colors. The shell calls
    /// this once on the active theme at load / hot-reload.
    pub fn downsample(&mut self, capability: ColorCapability) {
        macro_rules! adapt_fields {
            ($($field:ident),* $(,)?) => {
                $( self.$field = adapt_color(self.$field, capability); )*
            };
        }
        adapt_fields!(
            primary,
            secondary,
            accent,
            text,
            text_dim,
            text_bold,
            user_message,
            assistant_message,
            thinking,
            system_message,
            tool_running,
            tool_completed,
            tool_error,
            warning,
            success,
            error,
            border,
            border_focused,
            modal_border,
            panel_border,
            scrollbar,
            plan_mode,
            selection_bg,
            selection_fg,
            diff_added,
            diff_removed,
            code_keyword,
            code_string,
            code_comment,
            code_number,
            code_function,
            code_type,
            code_operator,
            code_inline,
            blockquote,
            heading,
            hr,
            strikethrough,
            hyperlink,
            table_border,
            table_header,
            search_match,
            progress_bar,
            context_used,
            context_free,
        );
        self.user_message_bg = self.user_message_bg.map(|c| adapt_color(c, capability));
        self.code_bg = self.code_bg.map(|c| adapt_color(c, capability));
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark_theme()
    }
}

#[cfg(test)]
#[path = "theme.test.rs"]
mod tests;
