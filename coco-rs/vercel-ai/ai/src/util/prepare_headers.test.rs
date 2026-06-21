use super::*;

#[test]
fn test_prepare_headers_empty() {
    let result = prepare_headers(None, None);
    assert!(result.is_empty());
}

#[test]
fn test_prepare_headers_base_only() {
    let mut base = HashMap::new();
    base.insert("Content-Type".to_string(), "application/json".to_string());

    let result = prepare_headers(Some(&base), None);

    assert_eq!(result.len(), 1);
    assert_eq!(
        result.get("Content-Type"),
        Some(&"application/json".to_string())
    );
}

#[test]
fn test_prepare_headers_combined() {
    let mut base = HashMap::new();
    base.insert("Content-Type".to_string(), "application/json".to_string());

    let mut additional = HashMap::new();
    additional.insert("X-Custom".to_string(), "value".to_string());

    let result = prepare_headers(Some(&base), Some(&additional));

    assert_eq!(result.len(), 2);
    assert_eq!(
        result.get("Content-Type"),
        Some(&"application/json".to_string())
    );
    assert_eq!(result.get("X-Custom"), Some(&"value".to_string()));
}

#[test]
fn test_prepare_headers_override() {
    let mut base = HashMap::new();
    base.insert("X-Key".to_string(), "base-value".to_string());

    let mut additional = HashMap::new();
    additional.insert("X-Key".to_string(), "override-value".to_string());

    let result = prepare_headers(Some(&base), Some(&additional));

    assert_eq!(result.get("X-Key"), Some(&"override-value".to_string()));
}

#[test]
fn test_prepare_headers_with_auth() {
    let result = prepare_headers_with_auth("my-api-key", None);

    assert_eq!(
        result.get("Authorization"),
        Some(&"Bearer my-api-key".to_string())
    );
}

#[test]
fn test_prepare_provider_headers_anthropic() {
    let result = prepare_provider_headers("anthropic", "my-key", None);

    assert_eq!(result.get("x-api-key"), Some(&"my-key".to_string()));
    assert_eq!(
        result.get("anthropic-version"),
        Some(&"2023-06-01".to_string())
    );
    assert_eq!(
        result.get("Content-Type"),
        Some(&"application/json".to_string())
    );
}

#[test]
fn test_prepare_provider_headers_openai() {
    let result = prepare_provider_headers("openai", "my-key", None);

    assert_eq!(
        result.get("Authorization"),
        Some(&"Bearer my-key".to_string())
    );
    assert_eq!(
        result.get("Content-Type"),
        Some(&"application/json".to_string())
    );
}

#[test]
fn test_merge_headers() {
    let mut h1 = HashMap::new();
    h1.insert("A".to_string(), "1".to_string());

    let mut h2 = HashMap::new();
    h2.insert("B".to_string(), "2".to_string());

    let result = merge_headers(&[&h1, &h2]);

    assert_eq!(result.len(), 2);
    assert_eq!(result.get("A"), Some(&"1".to_string()));
    assert_eq!(result.get("B"), Some(&"2".to_string()));
}

#[test]
fn test_get_header_case_insensitive() {
    let mut headers = HashMap::new();
    headers.insert("Content-Type".to_string(), "application/json".to_string());

    assert_eq!(
        get_header(&headers, "Content-Type"),
        Some(&"application/json".to_string())
    );
    assert_eq!(
        get_header(&headers, "content-type"),
        Some(&"application/json".to_string())
    );
    assert_eq!(
        get_header(&headers, "CONTENT-TYPE"),
        Some(&"application/json".to_string())
    );
}
