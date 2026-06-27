use super::*;

#[test]
fn test_parse_retry_after_seconds() {
    assert_eq!(parse_retry_after("30"), Some(Duration::from_secs(30)));
    assert_eq!(parse_retry_after("0"), Some(Duration::from_secs(0)));
}

#[test]
fn test_parse_retry_after_invalid() {
    assert_eq!(parse_retry_after("invalid"), None);
}

#[test]
fn test_exponential_backoff() {
    let base = Duration::from_millis(100);
    let max = Duration::from_secs(60);

    assert_eq!(
        exponential_backoff(0, base, max),
        Duration::from_millis(100)
    );
    assert_eq!(
        exponential_backoff(1, base, max),
        Duration::from_millis(200)
    );
    assert_eq!(
        exponential_backoff(2, base, max),
        Duration::from_millis(400)
    );
    assert_eq!(
        exponential_backoff(3, base, max),
        Duration::from_millis(800)
    );
}

#[test]
fn test_exponential_backoff_max() {
    let base = Duration::from_millis(100);
    let max = Duration::from_millis(500);

    assert_eq!(
        exponential_backoff(3, base, max),
        Duration::from_millis(500)
    );
}
