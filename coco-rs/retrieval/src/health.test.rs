use super::*;
use tempfile::TempDir;

#[test]
fn test_metrics_avg_latency() {
    let metrics = IndexMetrics {
        search_latency_samples: vec![10, 20, 30, 40, 50],
        ..Default::default()
    };

    assert_eq!(metrics.avg_search_latency_ms(), 30.0);
}

#[test]
fn test_metrics_p99_latency() {
    let metrics = IndexMetrics {
        search_latency_samples: (1..=100).collect(),
        ..Default::default()
    };

    assert_eq!(metrics.p99_search_latency_ms(), 99);
}

#[test]
fn test_metrics_empty_latency() {
    let metrics = IndexMetrics::default();

    assert_eq!(metrics.avg_search_latency_ms(), 0.0);
    assert_eq!(metrics.p99_search_latency_ms(), 0);
}

#[test]
fn test_calculate_dir_size() {
    let dir = TempDir::new().unwrap();

    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "hello world").unwrap();

    let size = calculate_dir_size(dir.path());
    assert!(size > 0);
}

#[tokio::test]
async fn test_health_checker_no_stores() {
    let checker = HealthChecker::new();
    let status = checker.check().await.unwrap();

    assert!(!status.issues.is_empty());
    assert!(!status.vector_store_ok);
    assert!(!status.sqlite_ok);
}

#[tokio::test]
async fn test_health_checker_with_sqlite() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("test.db");
    let store = Arc::new(SqliteStore::open(&db_path).unwrap());

    let checker = HealthChecker::new().with_sqlite(store);
    let status = checker.check().await.unwrap();

    assert!(status.sqlite_ok);
    assert_eq!(status.file_count, 0);
}

#[tokio::test]
async fn test_repairer_non_repairable() {
    let repairer = IndexRepairer::new();
    let issue = HealthIssue {
        severity: IssueSeverity::Critical,
        category: IssueCategory::Database,
        message: "Test issue".to_string(),
        repairable: false,
    };

    let result = repairer.repair(&issue).await.unwrap();
    assert!(!result.success);
    assert_eq!(result.repaired_count, 0);
}

#[tokio::test]
async fn test_metrics_collector_with_sqlite() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("test.db");
    let store = Arc::new(SqliteStore::open(&db_path).unwrap());

    let collector = MetricsCollector::new()
        .with_sqlite(store)
        .with_data_dir(dir.path());

    let metrics = collector.collect().await.unwrap();

    assert_eq!(metrics.total_files, 0);
    assert_eq!(metrics.total_chunks, 0);
}
