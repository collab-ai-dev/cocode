//! TUI module for the retrieval system.
//!
//! Provides an interactive terminal user interface for:
//! - Searching code with live results
//! - Managing index builds
//! - Exploring RepoMap rankings
//! - Monitoring file changes
//! - Debugging search pipeline
//!
//! # Architecture
//!
//! The TUI follows the same patterns as `codex-tui2`:
//! - Event-driven state management (`AppEvent`)
//! - View-based organization
//! - Composable widgets
//!
//! # Usage
//!
//! ```ignore
//! use coco_retrieval::tui::run_tui;
//! use coco_retrieval::config::RetrievalConfig;
//!
//! #[tokio::main]
//! async fn main() -> crate::Result<()> {
//!     let config = RetrievalConfig::load(&std::env::current_dir()?, &coco_home)?;
//!     run_tui(config).await
//! }
//! ```

pub mod app;
pub mod app_event;
pub mod app_state;
pub mod constants;
mod handlers;
mod render;
pub mod terminal;

// View modules
pub mod views;

// Widget modules
pub mod widgets;

// Re-exports
pub use app::App;
pub use app_event::AppEvent;
pub use app_event::ViewMode;
pub use terminal::TerminalGuard;
pub use terminal::Tui;
pub use terminal::init as init_terminal;
pub use terminal::restore as restore_terminal;

use std::sync::Arc;

use crate::config::RetrievalConfig;
use crate::facade::RetrievalFacade;

/// Run the TUI application with the given configuration.
///
/// This is the main entry point for the TUI mode.
///
/// # Arguments
/// * `config` - Retrieval configuration
/// * `service` - Optional pre-initialized RetrievalFacade. If None, the TUI
///   will be display-only and cannot perform operations.
pub async fn run_tui(
    config: RetrievalConfig,
    service: Option<Arc<RetrievalFacade>>,
) -> crate::Result<()> {
    // Initialize terminal
    let mut terminal = init_terminal()?;

    // Create app with optional service
    let mut app = App::new(config, service);

    // Run the app
    let result = app.run(&mut terminal).await;

    // Restore terminal (always, even on error)
    restore_terminal()?;

    result
}

/// Run the TUI with default configuration.
///
/// Loads configuration from the current directory.
/// Note: This creates a display-only TUI without a service.
/// Use `run_tui()` with a service parameter for full functionality.
pub async fn run_tui_default() -> crate::Result<()> {
    let workdir = std::env::current_dir()?;
    let coco_home = crate::find_coco_home();
    let config = RetrievalConfig::load(&workdir, &coco_home)?;
    run_tui(config, None).await
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
