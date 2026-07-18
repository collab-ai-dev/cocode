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

use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

use anyhow::Result;
use tokio::sync::{Mutex, RwLock, mpsc};
use tracing::{debug, info, warn};

use coco_config::{EnvKey, env};
use coco_query::{CoreEvent, QueuedImage, ServerNotification};
use coco_tui::{App, UserCommand, app::create_channels};
use coco_types::{SlashCommandStatusKind, TuiOnlyEvent};
use tokio_util::sync::CancellationToken;

use coco_agent_host::{resume_resolver::ResumePlan, session_bootstrap::build_engine_resources};
use coco_app_runtime::ProcessRuntime;

mod bootstrap;
mod driver;
mod editor_workflows;
mod goal_commands;
mod model_controls;
mod observability_commands;
mod plugin_dialog;
mod provider_commands;
mod session_search;
mod session_switching;
mod slash_execution;
mod slash_resolution;
mod teammate_inbox_pump;
mod turn_operations;
mod turn_postprocessing;
mod voice_bootstrap;

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
    bridge: &coco_agent_host::app_server_host::AppServerLocalBridge,
) -> &coco_agent_host::local_client::LocalSessionClient {
    let Some(session) = bridge.interactive_session() else {
        panic!("TUI command requires an attached interactive AppServer surface");
    };
    session
}

fn interactive_target(
    bridge: &coco_agent_host::app_server_host::AppServerLocalBridge,
) -> coco_types::InteractiveTarget {
    interactive_session(bridge).interactive_target()
}

fn session_target(
    bridge: &coco_agent_host::app_server_host::AppServerLocalBridge,
) -> coco_types::SessionTarget {
    interactive_session(bridge).session_target()
}

fn interactive_session_for<'a>(
    bridge: &'a coco_agent_host::app_server_host::AppServerLocalBridge,
    session_id: &coco_types::SessionId,
) -> Option<&'a coco_agent_host::local_client::LocalSessionClient> {
    bridge.interactive_session_by_id(session_id)
}

fn session_target_for(
    bridge: &coco_agent_host::app_server_host::AppServerLocalBridge,
    session_id: &coco_types::SessionId,
) -> Option<coco_types::SessionTarget> {
    interactive_session_for(bridge, session_id)
        .map(coco_agent_host::local_client::LocalSessionClient::session_target)
}

/// Adapt engine events emitted outside an AppServer surface (for example the
/// localized slash-result writeback) into the same exact-session envelope used
/// by surface pumps.
fn session_scoped_event_sender(
    event_tx: &mpsc::Sender<coco_types::CoreEvent>,
    session_id: coco_types::SessionId,
) -> mpsc::Sender<coco_types::CoreEvent> {
    let (scoped_tx, mut scoped_rx) = mpsc::channel(16);
    let event_tx = event_tx.clone();
    tokio::spawn(async move {
        while let Some(event) = scoped_rx.recv().await {
            let Ok(event) = coco_types::SessionScopedEvent::try_from(event) else {
                tracing::warn!(%session_id, "dropping non-scopeable local session event");
                continue;
            };
            if event_tx
                .send(coco_types::CoreEvent::Tui(
                    coco_types::TuiOnlyEvent::SessionScoped {
                        session_id: session_id.clone(),
                        event: Box::new(event),
                    },
                ))
                .await
                .is_err()
            {
                break;
            }
        }
    });
    scoped_tx
}

#[cfg(test)]
#[path = "tui.test.rs"]
mod tests;
