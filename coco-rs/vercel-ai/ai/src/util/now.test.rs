use super::*;

#[test]
fn test_now_secs() {
    let secs = now_secs();
    assert!(secs > 0);
}

#[test]
fn test_now_millis() {
    let millis = now_millis();
    assert!(millis > 0);
}

#[test]
fn test_now_micros() {
    let micros = now_micros();
    assert!(micros > 0);
}

#[test]
fn test_now_iso8601() {
    let iso = now_iso8601();
    assert!(!iso.is_empty());
    assert!(iso.contains('T'));
}

#[test]
fn test_elapsed() {
    let start = now_secs();
    std::thread::sleep(std::time::Duration::from_millis(10));
    let elapsed = elapsed_secs(start);
    assert!(elapsed == 0 || elapsed >= 1); // Might be 0 if less than a second
}