//! Presentation primitives for TUI surfaces.
//!
//! This module is intentionally crate-private. Domain state and external
//! commands stay in `state`, `update`, and `command`; presentation owns only
//! view models, layout guards, and rendering helpers.

pub(crate) mod activity;
pub(crate) mod add_directory;
pub(crate) mod agents_dialog;
pub(crate) mod confirm;
pub(crate) mod context_usage;
pub(crate) mod context_view;
pub(crate) mod header;
pub(crate) mod help;
pub(crate) mod information;
pub(crate) mod input;
pub(crate) mod layout;
pub(crate) mod model_picker;
pub(crate) mod pager;
pub(crate) mod permissions_editor;
pub(crate) mod picker;
pub(crate) mod picker_styled;
pub(crate) mod plan;
pub(crate) mod request;
pub(crate) mod rewind;
pub(crate) mod settings;
pub(crate) mod slash_command;
pub(crate) mod streaming;
pub(crate) mod theme_picker;
pub(crate) mod thinking;
pub(crate) mod transcript;
