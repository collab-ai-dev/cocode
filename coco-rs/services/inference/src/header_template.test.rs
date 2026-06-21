use super::*;
use pretty_assertions::assert_eq;

fn per_build() -> PerBuildVars {
    PerBuildVars {
        provider: "my-gateway".to_string(),
        model_id: "claude-opus-4-8".to_string(),
        api: "anthropic",
        base_url: "https://gw.example.com/v1".to_string(),
        account_kind: "api_key",
    }
}

fn vars() -> HeaderVars {
    HeaderVars {
        session_id: "sess-abc-123".to_string(),
        cwd: "/work/proj".to_string(),
        app_version: "9.9.9".to_string(),
    }
}

#[test]
fn literal_passthrough_when_no_dollar() {
    let pb = per_build();
    assert_eq!(
        expand("plain-value", Some(&vars()), &pb).unwrap(),
        "plain-value"
    );
}

#[test]
fn resolves_session_and_per_build_vars() {
    let pb = per_build();
    let v = vars();
    assert_eq!(
        expand("${SESSION_ID}", Some(&v), &pb).unwrap(),
        "sess-abc-123"
    );
    assert_eq!(expand("${CWD}", Some(&v), &pb).unwrap(), "/work/proj");
    assert_eq!(expand("${APP_VERSION}", Some(&v), &pb).unwrap(), "9.9.9");
    assert_eq!(expand("${PROVIDER}", Some(&v), &pb).unwrap(), "my-gateway");
    assert_eq!(
        expand("${MODEL_ID}", Some(&v), &pb).unwrap(),
        "claude-opus-4-8"
    );
    assert_eq!(expand("${API}", Some(&v), &pb).unwrap(), "anthropic");
    assert_eq!(
        expand("${BASE_URL}", Some(&v), &pb).unwrap(),
        "https://gw.example.com/v1"
    );
    assert_eq!(expand("${ACCOUNT_KIND}", Some(&v), &pb).unwrap(), "api_key");
}

#[test]
fn os_and_arch_are_non_empty_without_session() {
    let pb = per_build();
    assert!(!expand("${OS}", None, &pb).unwrap().is_empty());
    assert!(!expand("${ARCH}", None, &pb).unwrap().is_empty());
}

#[test]
fn json_template_needs_no_brace_escaping() {
    // The `{` `}` `"` `:` `,` of the JSON document are all literals — only the
    // two `${...}` sites expand.
    let pb = per_build();
    let v = vars();
    let out = expand(
        r#"{"sid":"${SESSION_ID}","model":"${MODEL_ID}"}"#,
        Some(&v),
        &pb,
    )
    .unwrap();
    assert_eq!(out, r#"{"sid":"sess-abc-123","model":"claude-opus-4-8"}"#);
}

#[test]
fn bare_dollar_is_literal() {
    let pb = per_build();
    let v = vars();
    // No braces ⇒ not a variable.
    assert_eq!(expand("$SESSION_ID", Some(&v), &pb).unwrap(), "$SESSION_ID");
    assert_eq!(
        expand("cost is $5 today", None, &pb).unwrap(),
        "cost is $5 today"
    );
    // A JSON `$ref` key is untouched (the `#` needs `r##"…"##` delimiters).
    assert_eq!(
        expand(r##"{"$ref":"#/x"}"##, None, &pb).unwrap(),
        r##"{"$ref":"#/x"}"##
    );
    // Trailing lone `$`.
    assert_eq!(expand("ends-with-$", None, &pb).unwrap(), "ends-with-$");
}

#[test]
fn double_dollar_escapes_to_literal() {
    let pb = per_build();
    let v = vars();
    assert_eq!(expand("$$", None, &pb).unwrap(), "$");
    // `$${X}` emits a literal `${X}` rather than expanding.
    assert_eq!(
        expand("$${SESSION_ID}", Some(&v), &pb).unwrap(),
        "${SESSION_ID}"
    );
}

#[test]
fn concatenation_and_braces_compose() {
    let pb = per_build();
    let v = vars();
    assert_eq!(
        expand("${PROVIDER}/${MODEL_ID}/${SESSION_ID}", Some(&v), &pb).unwrap(),
        "my-gateway/claude-opus-4-8/sess-abc-123"
    );
    // Braces let a var abut following identifier chars.
    assert_eq!(
        expand("${SESSION_ID}suffix", Some(&v), &pb).unwrap(),
        "sess-abc-123suffix"
    );
}

#[test]
fn unknown_variable_is_error() {
    let pb = per_build();
    let err = expand("${NOPE}", Some(&vars()), &pb).unwrap_err();
    assert_eq!(
        err,
        TemplateError::UnknownVariable {
            name: "NOPE".to_string()
        }
    );
}

#[test]
fn unterminated_placeholder_is_error() {
    let pb = per_build();
    assert_eq!(
        expand("${SESSION_ID", Some(&vars()), &pb).unwrap_err(),
        TemplateError::UnterminatedPlaceholder
    );
}

#[test]
fn session_var_without_context_resolves_empty() {
    let pb = per_build();
    // No HeaderVars ⇒ session-scoped names degrade to "" (not an error), while
    // per-build names still resolve.
    assert_eq!(
        expand("[${SESSION_ID}]${PROVIDER}", None, &pb).unwrap(),
        "[]my-gateway"
    );
}

#[test]
fn env_passthrough_missing_var_is_empty() {
    let pb = per_build();
    // Reading a (near-certainly) unset env var resolves to "" — no process-env
    // mutation needed, so the test is hermetic.
    assert_eq!(
        expand("${ENV:COCO_DEFINITELY_UNSET_HEADER_VAR_XYZ}", None, &pb).unwrap(),
        ""
    );
}

#[test]
fn crlf_is_stripped_from_expanded_value() {
    let pb = per_build();
    assert_eq!(expand("a\r\nb", None, &pb).unwrap(), "ab");
}
