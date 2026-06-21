use std::time::Duration;

use super::*;

#[test]
fn test_exponential_backoff() {
    let config = RetryConfig {
        max_retries: 5,
        base_delay_ms: 1000,
        max_delay_ms: 60_000,
        jitter_factor: 0.0, // no jitter for deterministic tests
    };
    let err = crate::errors::NetworkSnafu {
        message: "timeout".to_string(),
    }
    .build();

    assert_eq!(
        config.delay_for_attempt(0, &err),
        Duration::from_millis(1000)
    ); // 1000 * 2^0
    assert_eq!(
        config.delay_for_attempt(1, &err),
        Duration::from_millis(2000)
    ); // 1000 * 2^1
    assert_eq!(
        config.delay_for_attempt(2, &err),
        Duration::from_millis(4000)
    ); // 1000 * 2^2
    assert_eq!(
        config.delay_for_attempt(3, &err),
        Duration::from_millis(8000)
    ); // 1000 * 2^3
}

#[test]
fn test_backoff_capped_at_max() {
    let config = RetryConfig {
        max_retries: 10,
        base_delay_ms: 1000,
        max_delay_ms: 5000,
        jitter_factor: 0.0,
    };
    let err = crate::errors::NetworkSnafu {
        message: "timeout".to_string(),
    }
    .build();

    // 1000 * 2^5 = 32000, but capped at 5000
    assert_eq!(
        config.delay_for_attempt(5, &err),
        Duration::from_millis(5000)
    );
}

#[test]
fn test_server_retry_after_takes_priority() {
    let config = RetryConfig::default();
    let err = crate::errors::RateLimitedSnafu {
        retry_after_ms: Some(15000_i64),
        message: "slow down".to_string(),
    }
    .build();

    // Should use server's retry-after, not calculated backoff
    assert_eq!(
        config.delay_for_attempt(0, &err),
        Duration::from_millis(15000)
    );
}

#[test]
fn test_should_retry_within_limit() {
    let config = RetryConfig {
        max_retries: 3,
        ..Default::default()
    };
    let retryable = crate::errors::NetworkSnafu {
        message: "err".to_string(),
    }
    .build();
    let non_retryable = crate::errors::AuthenticationFailedSnafu {
        message: "err".to_string(),
    }
    .build();

    assert!(config.should_retry(0, &retryable));
    assert!(config.should_retry(2, &retryable));
    assert!(!config.should_retry(3, &retryable)); // at limit
    assert!(!config.should_retry(0, &non_retryable)); // not retryable
}

#[test]
fn test_overload_cascade_capped_but_others_get_full_budget() {
    let config = RetryConfig {
        max_retries: 10,
        ..Default::default()
    };
    // Overload cascade (503/529) — capped at MAX_CAPACITY_RETRIES (3) so the
    // fallback chain engages fast, even though max_retries is 10.
    let overloaded = crate::errors::OverloadedSnafu {
        retry_after_ms: None,
    }
    .build();
    assert!(config.should_retry(2, &overloaded));
    assert!(
        !config.should_retry(3, &overloaded),
        "overload cascade must cap at 3 regardless of max_retries"
    );

    // Rate limit (429) and generic network/5xx — NOT capacity-capped: full
    // budget (429 / status>=500 up to DEFAULT_MAX_RETRIES).
    let rate_limited = crate::errors::RateLimitedSnafu {
        retry_after_ms: None,
        message: "slow down".to_string(),
    }
    .build();
    let network = crate::errors::NetworkSnafu {
        message: "5xx".to_string(),
    }
    .build();
    // Background sources throw immediately on a capacity cascade;
    // foreground / untagged sources still retry.
    assert!(
        !config.should_retry_with_source(0, &overloaded, Some("prompt_suggestion")),
        "background source must not retry on 529"
    );
    assert!(config.should_retry_with_source(0, &overloaded, Some("repl_main_thread")));
    assert!(config.should_retry_with_source(0, &overloaded, Some("compact")));
    assert!(config.should_retry_with_source(0, &overloaded, None));
    // A non-capacity error is unaffected by source gating.
    assert!(config.should_retry_with_source(0, &rate_limited, Some("prompt_suggestion")));

    assert!(config.should_retry(5, &rate_limited));
    assert!(config.should_retry(9, &rate_limited));
    assert!(config.should_retry(9, &network));
    assert!(!config.should_retry(10, &network)); // at max_retries
}

#[test]
fn test_jitter_adds_delay() {
    let config = RetryConfig {
        max_retries: 3,
        base_delay_ms: 1000,
        max_delay_ms: 60_000,
        jitter_factor: 0.5,
    };
    let err = crate::errors::NetworkSnafu {
        message: "err".to_string(),
    }
    .build();

    // #136: jitter is now random in [0, jitter_factor*delay]. With 0.5
    // jitter on a 1000ms base, the result lies in [1000, 1500]. Sample a
    // few times so a degenerate constant 0 / constant max would surface.
    let mut saw_below_max = false;
    for _ in 0..50 {
        let d = config.delay_for_attempt(0, &err).as_millis() as u64;
        assert!((1000..=1500).contains(&d), "jitter out of range: {d}");
        if d < 1500 {
            saw_below_max = true;
        }
    }
    assert!(
        saw_below_max,
        "jitter should vary below the max, not be a fixed +50%"
    );
}
