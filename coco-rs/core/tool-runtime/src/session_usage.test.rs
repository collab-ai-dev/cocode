use std::sync::Arc;

use super::*;

#[test]
fn invalid_limits_clamp_to_one() {
    let limits = SessionUsageLimits::new(0, -5);

    assert!(matches!(
        limits.try_record_agent_spawn(),
        UsageLimitDecision::Allowed { used: 1, limit: 1 }
    ));
    assert!(matches!(
        limits.try_record_agent_spawn(),
        UsageLimitDecision::Exhausted { used: 1, limit: 1 }
    ));
    assert!(matches!(
        limits.try_record_web_search(),
        UsageLimitDecision::Allowed { used: 1, limit: 1 }
    ));
}

#[test]
fn concurrent_consumers_never_overshoot() {
    let limits = Arc::new(SessionUsageLimits::new(7, 3));
    let threads: [std::thread::JoinHandle<UsageLimitDecision>; 64] = std::array::from_fn(|_| {
        let limits = Arc::clone(&limits);
        std::thread::spawn(move || limits.try_record_agent_spawn())
    });

    let allowed = threads
        .into_iter()
        .map(|thread| thread.join().expect("limit worker must not panic"))
        .filter(|decision| matches!(decision, UsageLimitDecision::Allowed { .. }))
        .count();

    assert_eq!(allowed, 7);
    assert_eq!(limits.agent_spawns(), 7);
}
