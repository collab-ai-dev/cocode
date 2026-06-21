use super::*;

#[test]
fn test_command_suggestions() {
    let commands = &["help", "compact", "config", "clear", "commit"];
    let suggestions = get_command_suggestions("c", commands);
    assert!(suggestions.len() >= 3); // compact, config, clear, commit
    assert!(suggestions.iter().all(|s| s.text.starts_with("/c")));
}

#[test]
fn test_command_suggestions_empty_prefix() {
    let commands = &["help", "status"];
    let suggestions = get_command_suggestions("", commands);
    assert_eq!(suggestions.len(), 2);
}
