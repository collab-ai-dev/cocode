use super::structured_output_failure_limit_reached;
use super::structured_output_recall_limit_reached;
use crate::engine::RunArtifacts;

#[test]
fn structured_output_recall_limit_requires_contract_success_and_third_attempt() {
    let mut artifacts = RunArtifacts {
        structured_output: Some(serde_json::json!({"answer": 1})),
        structured_output_attempts: 2,
        ..RunArtifacts::default()
    };
    assert!(!structured_output_recall_limit_reached(true, &artifacts));

    artifacts.structured_output_attempts = 3;
    assert!(structured_output_recall_limit_reached(true, &artifacts));
    assert!(!structured_output_recall_limit_reached(false, &artifacts));

    artifacts.structured_output = None;
    assert!(!structured_output_recall_limit_reached(true, &artifacts));
}

#[test]
fn structured_output_failure_limit_requires_no_success() {
    let mut artifacts = RunArtifacts {
        structured_output_failed_attempts: crate::config::max_structured_output_retries(),
        ..RunArtifacts::default()
    };
    assert!(structured_output_failure_limit_reached(&artifacts));

    artifacts.structured_output = Some(serde_json::json!({"answer": 1}));
    assert!(!structured_output_failure_limit_reached(&artifacts));
}
