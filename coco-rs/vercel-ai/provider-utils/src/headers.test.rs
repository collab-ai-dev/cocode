use super::*;

#[test]
fn test_combine_headers() {
    let h1 = Some({
        let mut h = HashMap::new();
        h.insert("a".to_string(), "1".to_string());
        h
    });
    let h2 = Some({
        let mut h = HashMap::new();
        h.insert("b".to_string(), "2".to_string());
        h
    });
    let combined = combine_headers(vec![h1, h2]);
    assert_eq!(combined.get("a"), Some(&"1".to_string()));
    assert_eq!(combined.get("b"), Some(&"2".to_string()));
}

#[test]
fn test_combine_headers_override() {
    let h1 = Some({
        let mut h = HashMap::new();
        h.insert("a".to_string(), "1".to_string());
        h
    });
    let h2 = Some({
        let mut h = HashMap::new();
        h.insert("a".to_string(), "2".to_string());
        h
    });
    let combined = combine_headers(vec![h1, h2]);
    assert_eq!(combined.get("a"), Some(&"2".to_string()));
}

#[test]
fn test_extract_header() {
    let mut headers = HashMap::new();
    headers.insert("Content-Type".to_string(), "application/json".to_string());
    assert_eq!(
        extract_header(&headers, "content-type"),
        Some("application/json")
    );
    assert_eq!(
        extract_header(&headers, "Content-Type"),
        Some("application/json")
    );
    assert_eq!(extract_header(&headers, "Authorization"), None);
}

#[test]
fn test_bearer_auth() {
    let headers = bearer_auth("test-token");
    assert_eq!(
        headers.get("authorization"),
        Some(&"Bearer test-token".to_string())
    );
}
