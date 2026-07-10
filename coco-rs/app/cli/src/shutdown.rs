use std::fmt;
use std::future::Future;
use std::time::Duration;

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

pub async fn drain_with_timeout<F, E>(timeout: Duration, drain: F) -> ShutdownDrainOutcome
where
    F: Future<Output = Result<(), E>>,
    E: fmt::Display,
{
    drain_with_timeout_or_signal(timeout, drain, std::future::pending()).await
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

pub async fn os_interrupt_signal() {
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
