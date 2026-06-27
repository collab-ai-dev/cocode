use super::NON_REBINDABLE;
use super::TERMINAL_RESERVED;
use super::get_reserved_shortcuts;
use super::lookup_reserved;
use super::normalize_key_for_comparison;
use crate::validator::Severity;

#[test]
fn non_rebindable_includes_ctrl_c_d_m() {
    let has = |k: &str| NON_REBINDABLE.iter().any(|r| r.key == k);
    assert!(has("ctrl+c"));
    assert!(has("ctrl+d"));
    assert!(has("ctrl+m"));
    for entry in NON_REBINDABLE {
        assert_eq!(entry.severity, Severity::Error);
    }
}

#[test]
fn terminal_reserved_includes_ctrl_z_and_backslash() {
    let has = |k: &str| TERMINAL_RESERVED.iter().any(|r| r.key == k);
    assert!(has("ctrl+z"));
    assert!(has("ctrl+\\"));
}

#[test]
fn normalize_collapses_aliases_and_sorts_modifiers() {
    assert_eq!(
        normalize_key_for_comparison("Ctrl+Shift+A"),
        normalize_key_for_comparison("shift+control+a"),
    );
    // Different alias spellings collapse to the same canonical form.
    assert_eq!(
        normalize_key_for_comparison("option+k"),
        normalize_key_for_comparison("alt+k"),
    );
    assert_eq!(
        normalize_key_for_comparison("command+x"),
        normalize_key_for_comparison("cmd+x"),
    );
}

#[test]
fn normalize_handles_chord_per_step() {
    // Splitting a chord on `+` first would mangle "x ctrl" into a
    // modifier list — verify per-step normalization is applied.
    let normalized = normalize_key_for_comparison("ctrl+x ctrl+b");
    assert_eq!(normalized, "ctrl+x ctrl+b");
}

#[test]
fn lookup_reserved_finds_ctrl_c() {
    let found = lookup_reserved("ctrl+c");
    assert!(found.is_some());
    let found = found.unwrap();
    assert_eq!(found.severity, Severity::Error);
    assert!(found.reason.contains("interrupt") || found.reason.contains("exit"));
}

#[test]
fn lookup_reserved_returns_none_for_safe_key() {
    assert!(lookup_reserved("ctrl+a").is_none());
}

#[test]
fn lookup_reserved_normalizes_input() {
    // Different spelling, same chord.
    assert!(lookup_reserved("Ctrl+C").is_some());
    assert!(lookup_reserved("control+c").is_some());
}

#[test]
fn get_reserved_includes_terminal_reserved() {
    let reserved = get_reserved_shortcuts();
    assert!(reserved.iter().any(|r| r.key == "ctrl+z"));
}

#[cfg(target_os = "macos")]
#[test]
fn macos_host_includes_macos_reserved() {
    let reserved = get_reserved_shortcuts();
    assert!(reserved.iter().any(|r| r.key == "cmd+c"));
    assert!(reserved.iter().any(|r| r.key == "cmd+space"));
}

#[cfg(not(target_os = "macos"))]
#[test]
fn non_macos_host_excludes_macos_reserved() {
    let reserved = get_reserved_shortcuts();
    assert!(reserved.iter().all(|r| r.key != "cmd+c"));
}
