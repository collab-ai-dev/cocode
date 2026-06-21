use super::*;
use crate::types::McpStdioConfig;
use pretty_assertions::assert_eq;

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn expands_known_var() {
    let lookup = env(&[("TOKEN", "secret")]);
    let mut missing = Vec::new();
    let out = expand_str("Bearer ${TOKEN}", &lookup, &mut missing);
    assert_eq!(out, "Bearer secret");
    assert!(missing.is_empty());
}

#[test]
fn uses_default_when_unset() {
    let lookup = env(&[]);
    let mut missing = Vec::new();
    let out = expand_str("${PORT:-8080}", &lookup, &mut missing);
    assert_eq!(out, "8080");
    assert!(missing.is_empty());
}

#[test]
fn set_var_overrides_default() {
    let lookup = env(&[("PORT", "9000")]);
    let mut missing = Vec::new();
    let out = expand_str("${PORT:-8080}", &lookup, &mut missing);
    assert_eq!(out, "9000");
}

#[test]
fn missing_var_left_literal_and_reported() {
    let lookup = env(&[]);
    let mut missing = Vec::new();
    let out = expand_str("${MY_TOKEN}", &lookup, &mut missing);
    assert_eq!(out, "${MY_TOKEN}");
    assert_eq!(missing, vec!["MY_TOKEN".to_string()]);
}

#[test]
fn default_may_contain_colon_dash() {
    let lookup = env(&[]);
    let mut missing = Vec::new();
    let out = expand_str("${X:-a:-b}", &lookup, &mut missing);
    assert_eq!(out, "a:-b");
}

#[test]
fn unterminated_placeholder_left_literal() {
    let lookup = env(&[]);
    let mut missing = Vec::new();
    let out = expand_str("a ${NOPE b", &lookup, &mut missing);
    assert_eq!(out, "a ${NOPE b");
    assert!(missing.is_empty());
}

#[test]
fn empty_placeholder_left_literal() {
    // `[^}]+` requires a non-empty body, so `${}` is not a placeholder.
    let lookup = env(&[]);
    let mut missing = Vec::new();
    let out = expand_str("x ${} y", &lookup, &mut missing);
    assert_eq!(out, "x ${} y");
    assert!(missing.is_empty());
}

#[test]
fn expands_value_after_unmatched_prefix() {
    let lookup = env(&[("A", "x")]);
    let mut missing = Vec::new();
    let out = expand_str("${} then ${A}", &lookup, &mut missing);
    assert_eq!(out, "${} then x");
    assert!(missing.is_empty());
}

#[test]
fn expands_full_stdio_config() {
    let lookup = env(&[("HOME", "/home/me"), ("TOKEN", "t")]);
    let mut config = McpServerConfig::Stdio(McpStdioConfig {
        command: "${HOME}/bin/server".to_string(),
        args: vec![
            "--token=${TOKEN}".to_string(),
            "--port=${PORT:-80}".to_string(),
        ],
        env: env(&[("AUTH", "Bearer ${TOKEN}")]),
        cwd: None,
    });
    let missing = expand_config(&mut config, &lookup);
    assert!(missing.is_empty());
    let McpServerConfig::Stdio(c) = config else {
        panic!("expected stdio");
    };
    assert_eq!(c.command, "/home/me/bin/server");
    assert_eq!(c.args, vec!["--token=t", "--port=80"]);
    assert_eq!(c.env.get("AUTH").unwrap(), "Bearer t");
}

#[test]
fn reports_each_missing_var_once() {
    let lookup = env(&[]);
    let mut config = McpServerConfig::Stdio(McpStdioConfig {
        command: "${A}".to_string(),
        args: vec!["${A}".to_string(), "${B}".to_string()],
        env: HashMap::new(),
        cwd: None,
    });
    let missing = expand_config(&mut config, &lookup);
    assert_eq!(missing, vec!["A".to_string(), "B".to_string()]);
}
