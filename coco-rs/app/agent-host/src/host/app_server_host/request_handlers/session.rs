//! `initialize`, session protocol endpoints, per-turn event forwarding, and
//! session-stat aggregation.

mod data;
mod delete;
mod events;
mod initialize;
mod labels;

pub(crate) use data::{handle_session_list, handle_session_read, handle_session_turns_list};
pub(crate) use delete::handle_session_delete;
pub(super) use events::forward_turn_events;
pub(crate) use initialize::handle_initialize;
pub(crate) use labels::{handle_session_rename, handle_session_toggle_tag};
