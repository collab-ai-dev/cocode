use super::*;

#[test]
fn test_server_health_display() {
    assert_eq!(format!("{}", ServerHealth::Healthy), "healthy");
    assert_eq!(format!("{}", ServerHealth::Crashed), "crashed");
    assert_eq!(format!("{}", ServerHealth::Failed), "failed");
}

#[test]
fn test_should_restart_enabled() {
    let config = LifecycleConfig {
        max_restarts: 3,
        restart_on_crash: true,
        ..Default::default()
    };
    let lifecycle = ServerLifecycle::new("test".to_string(), config);

    assert!(lifecycle.should_restart());
    lifecycle.increment_restart_count();
    assert!(lifecycle.should_restart());
    lifecycle.increment_restart_count();
    assert!(lifecycle.should_restart());
    lifecycle.increment_restart_count();
    assert!(!lifecycle.should_restart()); // Exceeded limit
}

#[test]
fn test_should_restart_disabled() {
    let config = LifecycleConfig {
        restart_on_crash: false,
        ..Default::default()
    };
    let lifecycle = ServerLifecycle::new("test".to_string(), config);

    assert!(!lifecycle.should_restart());
}

#[tokio::test]
async fn test_record_crash_with_restart() {
    let config = LifecycleConfig {
        max_restarts: 2,
        restart_on_crash: true,
        ..Default::default()
    };
    let lifecycle = ServerLifecycle::new("test".to_string(), config);

    // First crash - should restart
    assert!(lifecycle.record_crash().await);
    assert_eq!(lifecycle.health().await, ServerHealth::Crashed);

    // Simulate restart
    lifecycle.increment_restart_count();

    // Second crash - should restart
    assert!(lifecycle.record_crash().await);

    // Simulate restart
    lifecycle.increment_restart_count();

    // Third crash - should NOT restart (exceeded max)
    assert!(!lifecycle.record_crash().await);
    assert_eq!(lifecycle.health().await, ServerHealth::Failed);
}

#[tokio::test]
async fn test_record_started() {
    let lifecycle = ServerLifecycle::new("test".to_string(), LifecycleConfig::default());

    lifecycle.record_started().await;

    assert_eq!(lifecycle.health().await, ServerHealth::Healthy);
    let stats = lifecycle.stats().await;
    assert!(stats.started_at.is_some());
    assert!(stats.last_healthy.is_some());
    assert_eq!(stats.consecutive_crashes, 0);
}

#[tokio::test]
async fn test_record_healthy() {
    let lifecycle = ServerLifecycle::new("test".to_string(), LifecycleConfig::default());

    // Set to crashed first
    lifecycle.set_health(ServerHealth::Crashed).await;
    assert_eq!(lifecycle.health().await, ServerHealth::Crashed);

    // Record healthy
    lifecycle.record_healthy().await;
    assert_eq!(lifecycle.health().await, ServerHealth::Healthy);
}

#[test]
fn test_restart_count() {
    let lifecycle = ServerLifecycle::new("test".to_string(), LifecycleConfig::default());

    assert_eq!(lifecycle.get_restart_count(), 0);
    assert_eq!(lifecycle.increment_restart_count(), 1);
    assert_eq!(lifecycle.increment_restart_count(), 2);
    assert_eq!(lifecycle.get_restart_count(), 2);

    lifecycle.reset_restart_count();
    assert_eq!(lifecycle.get_restart_count(), 0);
}
