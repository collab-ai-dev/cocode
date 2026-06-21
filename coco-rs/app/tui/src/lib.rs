//! Terminal UI using Elm architecture (TEA) with ratatui.
//!
//! Architecture:
//! ```text
//! Model (AppState) ← Update (handle_command) ← Events ← View (render)
//! ```

// Load locale files at the crate root so the generated `_rust_i18n_t` symbol
// is visible to every `t!()` call across the crate. See `i18n` module for the
// init / locale-detection helpers.
rust_i18n::i18n!("locales", fallback = "en");

pub mod app;
pub mod autocomplete;
pub(crate) mod bottom_pane;
pub mod command;
pub(crate) mod completion;
pub(crate) mod copy;
pub(crate) mod cursor;
pub mod display_settings;
pub(crate) mod events;
mod frame_layout;
mod frame_requester;
pub(crate) mod git_index_watcher;
pub(crate) mod i18n;
pub(crate) mod job_control;
pub mod keybinding_bridge;
pub(crate) mod keybinding_dispatch;
pub(crate) mod keybinding_resolver;
pub mod keybinding_setup;
pub(crate) mod keyboard_modes;
pub(crate) mod keymap;
pub(crate) mod modal_pane;
mod perf;
pub(crate) mod permission_options;
mod presentation;
pub mod server_notification_handler;
pub mod state;
pub(crate) mod status_bar;
pub(crate) mod surface;
mod surface_content;
mod sync_update_probe;
mod system_theme_probe;
pub mod terminal;
pub mod theme;
pub mod tool_display;
pub mod transcript;
pub mod update;
mod update_rewind;
pub(crate) mod vim;
pub(crate) mod widgets;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

// ── Public API ──
pub use app::App;
pub use app::create_channels;
pub use coco_tui_ui::display::SyntaxHighlighting;
pub use coco_tui_ui::paste::ImageData;
pub use coco_tui_ui::paste::PasteManager;
pub use coco_tui_ui::paste::ResolvedInput;
pub use coco_tui_ui::style::UiStyles;
pub use command::SystemPushKind;
pub use command::UserCommand;
pub use display_settings::DisplaySettings;
pub use events::TuiCommand;
pub use events::TuiEvent;
pub use frame_layout::FrameLayout;
pub use server_notification_handler::handle_core_event;
#[cfg(any(test, feature = "testing"))]
pub use server_notification_handler::handle_event_for_test;
pub use state::AppState;
pub use state::FocusTarget;
pub use state::RunningState;
pub use terminal::Tui;
pub use terminal::restore_terminal;
pub use theme::Theme;
pub use theme::ThemeName;
