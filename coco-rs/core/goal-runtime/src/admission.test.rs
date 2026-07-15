use super::*;
use crate::test_support::sid;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn test_admission_bounds_and_releases() {
    let admission = AutonomousAdmission::new(1);
    assert_eq!(admission.available_permits(), 1);
    let permit = admission.acquire(&sid()).await;
    assert_eq!(admission.available_permits(), 0);
    drop(permit);
    assert_eq!(admission.available_permits(), 1);
}

#[tokio::test]
async fn test_admission_floor_of_one() {
    // A zero cap is clamped to one so autonomous work can never fully starve.
    let admission = AutonomousAdmission::new(0);
    assert_eq!(admission.available_permits(), 1);
}
