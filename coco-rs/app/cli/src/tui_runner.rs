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

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::Result;
use tokio::sync::{Mutex, RwLock, mpsc};
use tracing::{debug, info, warn};

use coco_config::{EnvKey, env};
use coco_query::{CoreEvent, QueuePriority, QueuedCommand, QueuedImage, ServerNotification};
use coco_system_reminder::QueueOrigin;
use coco_tui::{App, UserCommand, app::create_channels};
use coco_types::{SlashCommandStatusKind, TuiOnlyEvent};
use tokio_util::sync::CancellationToken;

use coco_agent_host::{
    resume_resolver::ResumePlan,
    session_bootstrap::{build_engine_resources, install_session_late_binds},
};
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

fn interactive_session(
    bridge: &coco_agent_host::sdk_server::AppServerLocalBridge,
) -> &coco_agent_host::local_client::LocalSessionClient {
    let Some(session) = bridge.interactive_session() else {
        panic!("TUI command requires an attached interactive AppServer surface");
    };
    session
}

fn interactive_target(
    bridge: &coco_agent_host::sdk_server::AppServerLocalBridge,
) -> coco_types::InteractiveTarget {
    interactive_session(bridge).interactive_target()
}

fn session_target(
    bridge: &coco_agent_host::sdk_server::AppServerLocalBridge,
) -> coco_types::SessionTarget {
    interactive_session(bridge).session_target()
}

#[cfg(test)]
#[path = "tui_runner.test.rs"]
mod tests;
