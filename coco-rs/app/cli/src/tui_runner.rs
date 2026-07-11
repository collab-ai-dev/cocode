//! TUI runner — orchestrates TUI ↔ QueryEngine ↔ FileHistory.
//!
//! Uses an explicit async task (`run_agent_driver`) since ratatui is not a
//! reactive framework.
//!
//! Architecture:
//! ```text
//! ┌─────────────┐ UserCommand ┌────────────────┐ LLM / tools ┌────────────┐
//! │ TUI App │ ──────────────>│ agent_driver │ ──────────────>│ QueryEngine│
//! │(ratatui) │ <──────────────│(tokio task) │ <──────────────│ │
//! └─────────────┘ ServerNotif. └────────────────┘ QueryEvent └────────────┘
//! │
//! FileHistoryState
//! ```

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use tracing::debug;
use tracing::info;
use tracing::warn;

use coco_config::EnvKey;
use coco_config::env;
use coco_query::CoreEvent;
use coco_query::QueuePriority;
use coco_query::QueuedCommand;
use coco_query::QueuedImage;
use coco_query::ServerNotification;
use coco_system_reminder::QueueOrigin;
use coco_tui::App;
use coco_tui::UserCommand;
use coco_tui::app::create_channels;
use coco_types::SlashCommandStatusKind;
use coco_types::TuiOnlyEvent;
use tokio_util::sync::CancellationToken;

use coco_agent_host::resume_resolver::ResumePlan;
use coco_agent_host::session_bootstrap::build_engine_resources;
use coco_agent_host::session_bootstrap::install_session_late_binds;
use coco_app_runtime::ProcessRuntime;

mod bootstrap;
mod driver;
mod editor_workflows;
mod goal_commands;
mod model_controls;
mod observability_commands;
mod plugin_dialog;
mod provider_commands;
mod session_switching;
mod slash_execution;
mod slash_resolution;
mod turn_operations;
mod turn_postprocessing;

pub use bootstrap::run_tui;

use bootstrap::TuiRuntimeReloadSubscriptions;
use driver::run_agent_driver;
use editor_workflows::*;
use goal_commands::*;
use model_controls::*;
use observability_commands::*;
use plugin_dialog::*;
use provider_commands::*;
use session_switching::*;
use slash_execution::*;
use slash_resolution::*;
use turn_operations::*;
use turn_postprocessing::*;

pub(super) type SharedSessionHandle = Arc<RwLock<crate::session_runtime::SessionHandle>>;

#[cfg(test)]
#[path = "tui_runner.test.rs"]
mod tests;
