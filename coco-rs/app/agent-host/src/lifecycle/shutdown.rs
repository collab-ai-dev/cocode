use std::fmt;
use std::future::Future;
use std::time::Duration;

use tokio::task::JoinHandle;

use crate::event_hub::ProcessEventHub;
use crate::session_runtime::SessionHandle;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShutdownDrainOutcome {
    Clean,
    Failed { message: String },
    TimedOut { timeout_secs: u64 },
    Interrupted,
}

impl ShutdownDrainOutcome {
    pub fn is_clean(&self) -> bool {
        matches!(self, Self::Clean)
    }
}

impl fmt::Display for ShutdownDrainOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Clean => f.write_str("clean"),
            Self::Failed { message } => write!(f, "failed: {message}"),
            Self::TimedOut { timeout_secs } => {
                write!(f, "timed out after {timeout_secs}s")
            }
            Self::Interrupted => f.write_str("interrupted by signal"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppServerShutdownOutcome {
    pub app_server: ShutdownDrainOutcome,
    pub event_hub: ShutdownDrainOutcome,
}

impl AppServerShutdownOutcome {
    pub fn clean() -> Self {
        Self {
            app_server: ShutdownDrainOutcome::Clean,
            event_hub: ShutdownDrainOutcome::Clean,
        }
    }

    pub fn is_clean(&self) -> bool {
        self.app_server.is_clean() && self.event_hub.is_clean()
    }

    pub fn into_result(self, component: &str) -> anyhow::Result<()> {
        shutdown_drain_result(&format!("{component} AppServer"), &self.app_server)?;
        shutdown_drain_result(&format!("{component} Event Hub"), &self.event_hub)
    }
}

#[derive(Clone, Copy)]
pub struct ShutdownCoordinator {
    component: &'static str,
    timeout: Duration,
}

impl ShutdownCoordinator {
    pub fn new(component: &'static str, timeout: Duration) -> Self {
        Self { component, timeout }
    }

    pub async fn drain_app_server<F, E>(&self, app_server_drain: F) -> ShutdownDrainOutcome
    where
        F: Future<Output = Result<(), E>>,
        E: fmt::Display,
    {
        drain_with_timeout_or_signal(self.timeout, app_server_drain, os_interrupt_signal()).await
    }

    pub async fn drain_app_server_and_event_hub<F, E>(
        &self,
        app_server_drain: F,
        event_hub_connector: Option<ProcessEventHub>,
        event_hub_membership_watcher: Option<JoinHandle<()>>,
    ) -> AppServerShutdownOutcome
    where
        F: Future<Output = Result<(), E>>,
        E: fmt::Display,
    {
        let app_server = self.drain_app_server(app_server_drain).await;
        self.finish_after_app_server(
            app_server,
            event_hub_connector,
            event_hub_membership_watcher,
        )
        .await
    }

    pub async fn finish_after_app_server(
        &self,
        app_server: ShutdownDrainOutcome,
        event_hub_connector: Option<ProcessEventHub>,
        event_hub_membership_watcher: Option<JoinHandle<()>>,
    ) -> AppServerShutdownOutcome {
        log_shutdown_outcome(self.component, "AppServer", &app_server);
        if let Some(handle) = event_hub_membership_watcher {
            handle.abort();
        }
        let event_hub = if let Some(connector) = event_hub_connector {
            connector
                .shutdown_and_flush_with_timeout(self.timeout)
                .await
        } else {
            ShutdownDrainOutcome::Clean
        };
        AppServerShutdownOutcome {
            app_server,
            event_hub,
        }
    }
}

pub fn shutdown_drain_result(
    component: &str,
    outcome: &ShutdownDrainOutcome,
) -> anyhow::Result<()> {
    if outcome.is_clean() {
        return Ok(());
    }
    Err(anyhow::anyhow!("{component} shutdown drain {outcome}"))
}

pub async fn drain_with_timeout<F, E>(timeout: Duration, drain: F) -> ShutdownDrainOutcome
where
    F: Future<Output = Result<(), E>>,
    E: fmt::Display,
{
    drain_with_timeout_or_signal(timeout, drain, std::future::pending()).await
}

fn log_shutdown_outcome(component: &str, phase: &str, outcome: &ShutdownDrainOutcome) {
    match outcome {
        ShutdownDrainOutcome::Clean => {}
        ShutdownDrainOutcome::Failed { message } => {
            tracing::warn!(component, phase, error = %message, "shutdown drain failed");
        }
        ShutdownDrainOutcome::TimedOut { timeout_secs } => {
            tracing::warn!(component, phase, timeout_secs, "shutdown drain timed out");
        }
        ShutdownDrainOutcome::Interrupted => {
            tracing::warn!(component, phase, "shutdown drain interrupted by signal");
        }
    }
}

/// Wait for scheduled turn-end extraction/session-memory work before process
/// exit so partial writes are not dropped.
pub async fn drain_session_memory(session: &SessionHandle) {
    if let Some(memory_runtime) = session.memory_runtime() {
        let _ = memory_runtime
            .drain(coco_memory::service::extract::DEFAULT_DRAIN_TIMEOUT)
            .await;
    }
}

/// Persist coordinator-mode metadata needed for a later resume.
pub async fn persist_session_resume_mode(session: &SessionHandle) {
    session.persist_session_mode().await;
}

/// Final interactive-session checkpoint after local AppServer shutdown.
pub async fn flush_full_session_exit_checkpoint(session: &SessionHandle) {
    session.re_append_session_metadata().await;
    session.persist_session_mode().await;
    session.flush_session_usage_snapshot().await;
}

pub async fn drain_with_timeout_or_signal<F, E, S>(
    timeout: Duration,
    drain: F,
    signal: S,
) -> ShutdownDrainOutcome
where
    F: Future<Output = Result<(), E>>,
    E: fmt::Display,
    S: Future<Output = ()>,
{
    tokio::select! {
        result = tokio::time::timeout(timeout, drain) => timeout_result_to_outcome(timeout, result),
        () = signal => ShutdownDrainOutcome::Interrupted,
    }
}

/// Resolves when the process receives an OS shutdown/interrupt signal, which
/// initiates the §7.7 graceful drain.
///
/// On Unix this observes both SIGINT (Ctrl+C) and SIGTERM — the latter is the
/// default `kill` signal and what init systems and container runtimes send, so
/// a plain `kill <pid>` drains cleanly instead of aborting mid-turn. On other
/// platforms only Ctrl+C is observed. If a handler fails to register the future
/// never resolves, so a registration error degrades to "no signal-initiated
/// shutdown" rather than a spurious immediate interrupt.
pub async fn os_interrupt_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        match signal(SignalKind::terminate()) {
            Ok(mut sigterm) => {
                tokio::select! {
                    () = ctrl_c_or_pending() => {}
                    _ = sigterm.recv() => {}
                }
            }
            Err(_) => ctrl_c_or_pending().await,
        }
    }
    #[cfg(not(unix))]
    {
        ctrl_c_or_pending().await;
    }
}

/// Waits for Ctrl+C, or never resolves if the handler cannot be installed —
/// preserving the pre-existing "registration failure is not a shutdown"
/// contract.
async fn ctrl_c_or_pending() {
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
}

fn timeout_result_to_outcome<E>(
    timeout: Duration,
    result: Result<Result<(), E>, tokio::time::error::Elapsed>,
) -> ShutdownDrainOutcome
where
    E: fmt::Display,
{
    match result {
        Ok(Ok(())) => ShutdownDrainOutcome::Clean,
        Ok(Err(error)) => ShutdownDrainOutcome::Failed {
            message: error.to_string(),
        },
        Err(_) => ShutdownDrainOutcome::TimedOut {
            timeout_secs: timeout.as_secs(),
        },
    }
}

#[cfg(test)]
#[path = "shutdown.test.rs"]
mod tests;
