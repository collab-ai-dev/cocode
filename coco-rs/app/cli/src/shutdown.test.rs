use std::time::Duration;

use super::ShutdownDrainOutcome;
use super::drain_with_timeout;
use super::drain_with_timeout_or_signal;

#[tokio::test]
async fn drain_with_timeout_returns_clean_on_success() {
    let outcome = drain_with_timeout(Duration::from_secs(1), async { Ok::<(), &str>(()) }).await;

    assert_eq!(outcome, ShutdownDrainOutcome::Clean);
}

#[tokio::test]
async fn drain_with_timeout_returns_failed_on_error() {
    let outcome = drain_with_timeout(Duration::from_secs(1), async {
        Err::<(), &str>("driver failed")
    })
    .await;

    assert_eq!(
        outcome,
        ShutdownDrainOutcome::Failed {
            message: "driver failed".to_string()
        }
    );
}

#[tokio::test]
async fn drain_with_timeout_returns_timed_out_after_deadline() {
    let outcome = drain_with_timeout(Duration::from_secs(0), async {
        std::future::pending::<()>().await;
        Ok::<(), &str>(())
    })
    .await;

    assert_eq!(outcome, ShutdownDrainOutcome::TimedOut { timeout_secs: 0 });
}

#[tokio::test]
async fn drain_with_timeout_or_signal_returns_interrupted_on_signal() {
    let outcome = drain_with_timeout_or_signal(
        Duration::from_secs(5),
        async {
            std::future::pending::<()>().await;
            Ok::<(), &str>(())
        },
        async {},
    )
    .await;

    assert_eq!(outcome, ShutdownDrainOutcome::Interrupted);
}
