use std::time::Duration;

use super::ShutdownDrainOutcome;
use super::drain_with_timeout;
use super::drain_with_timeout_or_signal;
use super::os_interrupt_signal;

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

#[tokio::test]
async fn os_interrupt_signal_does_not_resolve_without_a_signal() {
    // Registering the SIGINT/SIGTERM handlers must not itself resolve the
    // future — otherwise every shutdown drain would report `Interrupted`
    // immediately without an actual signal. A short poll window is enough to
    // catch a broken registration that resolves eagerly.
    let resolved = tokio::time::timeout(Duration::from_millis(100), os_interrupt_signal())
        .await
        .is_ok();

    assert!(
        !resolved,
        "os_interrupt_signal resolved without an OS signal"
    );
}
