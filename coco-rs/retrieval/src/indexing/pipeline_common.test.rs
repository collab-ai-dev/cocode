use super::*;

#[derive(Debug, Clone, PartialEq, Default)]
struct TestStats {
    count: i32,
}

#[test]
fn test_compute_readiness_uninitialized() {
    let state: PipelineState<TestStats> = PipelineState::Uninitialized;
    let lag_info = LagInfo::default();
    let strict_config = StrictModeConfig::default();

    let readiness = compute_readiness(&state, lag_info, false, &strict_config);
    assert!(matches!(readiness, PipelineReadiness::Uninitialized));
}

#[test]
fn test_compute_readiness_building() {
    let state: PipelineState<TestStats> = PipelineState::Building {
        batch_id: BatchId::new(),
        started_at: 12345,
        progress: 0.5,
    };
    let lag_info = LagInfo::default();
    let strict_config = StrictModeConfig::default();

    let readiness = compute_readiness(&state, lag_info, false, &strict_config);
    if let PipelineReadiness::Building { progress, .. } = readiness {
        assert_eq!(progress, 0.5);
    } else {
        panic!("Expected Building readiness");
    }
}

#[test]
fn test_compute_readiness_ready_no_lag() {
    let state: PipelineState<TestStats> = PipelineState::Ready {
        stats: TestStats { count: 10 },
        completed_at: 12345,
    };
    let lag_info = LagInfo::default(); // lag = 0
    let strict_config = StrictModeConfig::default();

    let readiness = compute_readiness(&state, lag_info, true, &strict_config);
    if let PipelineReadiness::Ready { stats, .. } = readiness {
        assert_eq!(stats.count, 10);
    } else {
        panic!("Expected Ready readiness");
    }
}

#[test]
fn test_compute_readiness_ready_with_lag_strict() {
    let state: PipelineState<TestStats> = PipelineState::Ready {
        stats: TestStats { count: 10 },
        completed_at: 12345,
    };
    let lag_info = LagInfo {
        lag: 5,
        ..Default::default()
    };
    let strict_config = StrictModeConfig {
        init: true,
        incremental: true, // strict mode for incremental
    };

    let readiness = compute_readiness(&state, lag_info, true, &strict_config);
    assert!(matches!(readiness, PipelineReadiness::NotReady { .. }));
}

#[test]
fn test_compute_readiness_ready_with_lag_not_strict() {
    let state: PipelineState<TestStats> = PipelineState::Ready {
        stats: TestStats { count: 10 },
        completed_at: 12345,
    };
    let lag_info = LagInfo {
        lag: 5,
        ..Default::default()
    };
    let strict_config = StrictModeConfig {
        init: true,
        incremental: false, // not strict for incremental
    };

    let readiness = compute_readiness(&state, lag_info, true, &strict_config);
    assert!(matches!(readiness, PipelineReadiness::Ready { .. }));
}

#[test]
fn test_compute_readiness_failed() {
    let state: PipelineState<TestStats> = PipelineState::Failed {
        error: "test error".to_string(),
        failed_at: 12345,
    };
    let lag_info = LagInfo::default();
    let strict_config = StrictModeConfig::default();

    let readiness = compute_readiness(&state, lag_info, false, &strict_config);
    if let PipelineReadiness::Failed { error } = readiness {
        assert_eq!(error, "test error");
    } else {
        panic!("Expected Failed readiness");
    }
}

#[test]
fn test_strict_mode_config_default() {
    let config = StrictModeConfig::default();
    assert!(config.init);
    assert!(!config.incremental);
}
