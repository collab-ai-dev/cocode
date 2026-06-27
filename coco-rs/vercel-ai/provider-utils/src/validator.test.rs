use super::*;

#[test]
fn test_validate_tool_name_valid() {
    assert!(validate_tool_name("search").is_ok());
    assert!(validate_tool_name("read_file").is_ok());
    assert!(validate_tool_name("write-file").is_ok());
    assert!(validate_tool_name("tool123").is_ok());
}

#[test]
fn test_validate_tool_name_invalid() {
    assert!(validate_tool_name("").is_err());
    assert!(validate_tool_name("tool with spaces").is_err());
    assert!(validate_tool_name("tool@name").is_err());
    assert!(validate_tool_name(&"a".repeat(65)).is_err());
}

#[test]
fn test_validate_model_id_valid() {
    assert!(validate_model_id("gpt-4").is_ok());
    assert!(validate_model_id("claude-3-opus").is_ok());
    assert!(validate_model_id("models/gemini-pro").is_ok());
    assert!(validate_model_id("model:v1.0.0").is_ok());
}

#[test]
fn test_validate_model_id_invalid() {
    assert!(validate_model_id("").is_err());
    assert!(validate_model_id("model with spaces").is_err());
    assert!(validate_model_id(&"a".repeat(129)).is_err());
}

#[test]
fn test_validate_url_valid() {
    assert!(validate_url("https://api.example.com").is_ok());
    assert!(validate_url("http://localhost:8080/path").is_ok());
}

#[test]
fn test_validate_url_invalid() {
    assert!(validate_url("").is_err());
    assert!(validate_url("not-a-url").is_err());
    assert!(validate_url("ftp://example.com").is_err());
}

#[test]
fn test_validate_api_key_valid() {
    assert!(validate_api_key("sk-1234567890abcdef").is_ok());
    assert!(validate_api_key("real_api_key_123").is_ok());
}

#[test]
fn test_validate_api_key_invalid() {
    assert!(validate_api_key("").is_err());
    assert!(validate_api_key("key with spaces").is_err());
    assert!(validate_api_key("your-api-key").is_err());
    assert!(validate_api_key("sk-xxx").is_err());
    assert!(validate_api_key("test").is_err());
}
