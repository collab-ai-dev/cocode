use super::*;

// `on`/`off`/`toggle` persist to the process-global user settings.json, so
// they are exercised via integration paths rather than here (a unit test must
// not mutate the real config). The argument-parsing / usage branch is pure.

#[test]
fn unknown_argument_returns_usage_without_persisting() {
    let out = run("sideways").expect("ok");
    assert!(out.contains("Unknown argument"));
    assert!(out.contains("/voice [on|off|toggle]"));
}
