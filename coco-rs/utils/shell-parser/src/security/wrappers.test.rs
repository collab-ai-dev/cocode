use super::*;
use pretty_assertions::assert_eq;

fn args(s: &str) -> Vec<String> {
    s.split_whitespace().map(String::from).collect()
}

// -- time --

#[test]
fn test_strip_time() {
    assert_eq!(strip_wrappers(&args("time ls -la")), Some(args("ls -la")));
}

#[test]
fn test_strip_time_bare() {
    assert_eq!(strip_wrappers(&args("time")), None);
}

// -- nohup --

#[test]
fn test_strip_nohup() {
    assert_eq!(
        strip_wrappers(&args("nohup curl http://example.com")),
        Some(args("curl http://example.com"))
    );
}

#[test]
fn test_strip_nohup_bare() {
    assert_eq!(strip_wrappers(&args("nohup")), None);
}

// -- timeout --

#[test]
fn test_strip_timeout_simple() {
    assert_eq!(strip_wrappers(&args("timeout 5 ls")), Some(args("ls")));
}

#[test]
fn test_strip_timeout_with_unit() {
    assert_eq!(
        strip_wrappers(&args("timeout 30s ls -la")),
        Some(args("ls -la"))
    );
}

#[test]
fn test_strip_timeout_with_flags() {
    assert_eq!(
        strip_wrappers(&args("timeout --foreground -v 10 cmd arg")),
        Some(args("cmd arg"))
    );
}

#[test]
fn test_strip_timeout_kill_after_fused() {
    assert_eq!(
        strip_wrappers(&args("timeout --kill-after=5 10 cmd")),
        Some(args("cmd"))
    );
}

#[test]
fn test_strip_timeout_signal_separate() {
    assert_eq!(
        strip_wrappers(&args("timeout --signal TERM 10 cmd")),
        Some(args("cmd"))
    );
}

#[test]
fn test_strip_timeout_short_k() {
    assert_eq!(
        strip_wrappers(&args("timeout -k 5 10 cmd")),
        Some(args("cmd"))
    );
}

#[test]
fn test_strip_timeout_unknown_flag_fails() {
    assert_eq!(strip_wrappers(&args("timeout --unknown 10 cmd")), None);
}

#[test]
fn test_strip_timeout_no_duration_fails() {
    assert_eq!(strip_wrappers(&args("timeout -v")), None);
}

// -- nice --

#[test]
fn test_strip_nice_simple() {
    assert_eq!(strip_wrappers(&args("nice ls")), Some(args("ls")));
}

#[test]
fn test_strip_nice_n_flag() {
    assert_eq!(strip_wrappers(&args("nice -n 10 cmd")), Some(args("cmd")));
}

#[test]
fn test_strip_nice_legacy_numeric() {
    assert_eq!(strip_wrappers(&args("nice -10 cmd")), Some(args("cmd")));
}

#[test]
fn test_strip_nice_negative() {
    assert_eq!(strip_wrappers(&args("nice -n -5 cmd")), Some(args("cmd")));
}

#[test]
fn test_strip_nice_rejects_expansion() {
    assert_eq!(strip_wrappers(&args("nice $(evil) cmd")), None);
}

// -- env --

#[test]
fn test_strip_env_simple_assignment() {
    assert_eq!(strip_wrappers(&args("env FOO=bar cmd")), Some(args("cmd")));
}

#[test]
fn test_strip_env_multiple_assignments() {
    assert_eq!(
        strip_wrappers(&args("env FOO=bar BAZ=qux cmd arg")),
        Some(args("cmd arg"))
    );
}

#[test]
fn test_strip_env_with_i_flag() {
    assert_eq!(strip_wrappers(&args("env -i cmd")), Some(args("cmd")));
}

#[test]
fn test_strip_env_with_u_flag() {
    assert_eq!(strip_wrappers(&args("env -u VAR cmd")), Some(args("cmd")));
}

#[test]
fn test_strip_env_rejects_s_flag() {
    assert_eq!(strip_wrappers(&args("env -S cmd")), None);
}

#[test]
fn test_strip_env_rejects_c_flag() {
    assert_eq!(strip_wrappers(&args("env -C /tmp cmd")), None);
}

#[test]
fn test_strip_env_rejects_p_flag() {
    assert_eq!(strip_wrappers(&args("env -P /usr/bin cmd")), None);
}

#[test]
fn test_strip_env_rejects_long_flags() {
    assert_eq!(strip_wrappers(&args("env --split-string cmd")), None);
}

// -- stdbuf --

#[test]
fn test_strip_stdbuf_long_form() {
    assert_eq!(
        strip_wrappers(&args("stdbuf --output=L cmd")),
        Some(args("cmd"))
    );
}

#[test]
fn test_strip_stdbuf_short_fused() {
    assert_eq!(strip_wrappers(&args("stdbuf -oL cmd")), Some(args("cmd")));
}

#[test]
fn test_strip_stdbuf_short_separate() {
    assert_eq!(strip_wrappers(&args("stdbuf -o L cmd")), Some(args("cmd")));
}

#[test]
fn test_strip_stdbuf_multiple_flags() {
    assert_eq!(
        strip_wrappers(&args("stdbuf -oL -eL cmd arg")),
        Some(args("cmd arg"))
    );
}

#[test]
fn test_strip_stdbuf_unknown_flag_fails() {
    assert_eq!(strip_wrappers(&args("stdbuf --unknown cmd")), None);
}

// -- strip_all_wrappers --

#[test]
fn test_strip_nested_wrappers() {
    assert_eq!(
        strip_all_wrappers(&args("time nohup env FOO=bar cmd arg")),
        Some(args("cmd arg"))
    );
}

#[test]
fn test_strip_all_no_wrappers() {
    assert_eq!(strip_all_wrappers(&args("ls -la")), None);
}

// -- non-wrapper commands --

#[test]
fn test_non_wrapper_returns_none() {
    assert_eq!(strip_wrappers(&args("ls -la")), None);
    assert_eq!(strip_wrappers(&args("git status")), None);
    assert_eq!(strip_wrappers(&args("curl http://example.com")), None);
}
