use super::*;
use pretty_assertions::assert_eq;

#[test]
fn load_regex() {
    // Compile all regex patterns to catch syntax errors at test time.
    let _ = redact_secrets("nothing secret here");
}

#[test]
fn redact_openai_key() {
    let input = "export OPENAI_API_KEY=sk-abcdefghijklmnopqrstuvwxyz1234567890";
    let result = redact_secrets(input);
    assert!(
        !result.contains("sk-abcdefgh"),
        "OpenAI key should be redacted"
    );
    assert!(result.contains(REDACTED));
}

#[test]
fn redact_anthropic_key() {
    let input = "ANTHROPIC_API_KEY=sk-ant-api03-abcdefghijklmnopqrstuvwxyz";
    let result = redact_secrets(input);
    assert!(
        !result.contains("sk-ant-api03"),
        "Anthropic key should be redacted"
    );
    assert!(result.contains(REDACTED));
}

#[test]
fn redact_github_pat() {
    let input = "GH_TOKEN=ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmn";
    let result = redact_secrets(input);
    assert!(
        !result.contains("ghp_ABCDEF"),
        "GitHub PAT should be redacted"
    );
    assert!(result.contains(REDACTED));
}

#[test]
fn redact_github_fine_grained_pat() {
    let input = "github_pat_ABCDEFGHIJKLMNOPQRSTUVWXYZa";
    let result = redact_secrets(input);
    assert!(
        !result.contains("github_pat_ABC"),
        "GitHub fine-grained PAT should be redacted"
    );
}

#[test]
fn redact_slack_bot_token() {
    let input = "SLACK_TOKEN=xoxb-123456789-abcdefghijk";
    let result = redact_secrets(input);
    assert!(
        !result.contains("xoxb-123456789"),
        "Slack bot token should be redacted"
    );
}

#[test]
fn redact_slack_user_token() {
    let input = "xoxp-123456789-abcdefghijk";
    let result = redact_secrets(input);
    assert!(
        !result.contains("xoxp-123456789"),
        "Slack user token should be redacted"
    );
}

#[test]
fn redact_aws_access_key() {
    let input = "AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE";
    let result = redact_secrets(input);
    assert!(!result.contains("AKIAIOSFODNN7EXAMPLE"));
    assert!(result.contains(REDACTED));
}

#[test]
fn redact_bearer_token() {
    let input = "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9";
    let result = redact_secrets(input);
    assert!(!result.contains("eyJhbGci"));
    assert!(result.contains(REDACTED));
}

#[test]
fn redact_generic_secret_assignment() {
    let input = r#"api_key = "super_secret_value_12345""#;
    let result = redact_secrets(input);
    assert!(!result.contains("super_secret_value"));
    assert!(result.contains(REDACTED));
}

#[test]
fn no_false_positive_on_short_values() {
    let input = "token = abc";
    let result = redact_secrets(input);
    assert_eq!(&*result, input);
}

#[test]
fn no_false_positive_on_normal_text() {
    let input = "This is a normal log message with no secrets.";
    let result = redact_secrets(input);
    assert_eq!(&*result, input);
}

#[test]
fn zero_copy_when_no_secrets() {
    let input = "just a regular command output with no secrets";
    let result = redact_secrets(input);
    assert!(matches!(result, Cow::Borrowed(_)), "should be zero-copy");
}

#[test]
fn multiple_secrets_in_one_string() {
    let input = "key1=sk-aaaaaaaaaaaaaaaaaaaaaa key2=AKIAIOSFODNN7EXAMPLE";
    let result = redact_secrets(input);
    assert!(!result.contains("sk-aaa"));
    assert!(!result.contains("AKIAIOSFODNN7EXAMPLE"));
}

#[test]
fn anthropic_key_not_matched_as_openai() {
    let input = "sk-ant-api03-1234567890abcdefghijkl";
    let result = redact_secrets(input);
    assert!(!result.contains("ant-api03"));
    assert_eq!(result.matches(REDACTED).count(), 1);
}
