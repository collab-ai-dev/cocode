use super::*;

#[test]
fn test_iterm_backend_properties() {
    let backend = ITermBackend::new();
    assert_eq!(backend.backend_type(), BackendType::Iterm2);
    assert_eq!(backend.display_name(), "iTerm2");
    assert!(!backend.supports_hide_show());
}

#[test]
fn test_parse_split_output_valid() {
    let output = "Created new pane: abc-123-def\n";
    assert_eq!(parse_split_output(output), "abc-123-def");
}

#[test]
fn test_parse_split_output_empty() {
    assert_eq!(parse_split_output(""), "");
    assert_eq!(parse_split_output("some other output"), "");
}

#[test]
fn test_get_leader_session_id_format() {
    // This depends on env var, just verify no panic
    let _ = get_leader_session_id();
}

#[test]
fn test_it2_command_constant() {
    assert_eq!(IT2_COMMAND, "it2");
}

#[test]
fn test_iterm_backend_default() {
    let _backend = ITermBackend::default();
}
