//! TUI widgets — ratatui components for each UI section.
//!
//! Each widget implements the `ratatui::widgets::Widget` trait
//! and is constructed via builder pattern from immutable state references.
//!
//! Pure, domain-free widgets (textarea, markdown, notification, spinner_verbs,
//! status_indicator, diff_display) now live in `coco_tui_ui::widgets`.

mod activity_panel;
pub(crate) mod activity_summary;
mod agent_switcher;
mod background_pills;
pub mod error_dialog;
mod input;
mod queue_status_widget;
pub mod settings_panel;
mod stash_notice;
pub mod suggestion_popup;
mod toast;
pub(crate) mod todo_panel;
mod transcript_modal;

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;

pub(crate) use activity_panel::ActivityPanel;
pub(crate) use agent_switcher::AgentSwitcher;
pub(crate) use agent_switcher::build_view as build_agent_switcher_view;
pub(crate) use background_pills::BackgroundPills;
pub(crate) use background_pills::build_view as build_background_pills_view;
pub(crate) use input::HistorySearchView;
pub(crate) use input::InputRenderModel;
pub(crate) use input::InputWidget;
pub(crate) use input::scroll_offset;
pub(crate) use queue_status_widget::QueueStatusWidget;
pub(crate) use stash_notice::StashNotice;
pub(crate) use suggestion_popup::SuggestionPopup;
pub(crate) use toast::ToastWidget;
pub(crate) use transcript_modal::TranscriptLayoutIndex;
pub(crate) use transcript_modal::TranscriptStateWidget;
