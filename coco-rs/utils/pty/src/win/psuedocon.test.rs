use super::windows_build_number;
use super::MIN_CONPTY_BUILD;

#[test]
fn windows_build_number_returns_value() {
    // We can't stably check the version of the GH workers, but we can
    // at least check that this.
    let version = windows_build_number().unwrap();
    assert!(version > MIN_CONPTY_BUILD);
}
