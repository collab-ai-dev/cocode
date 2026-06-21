use super::*;
use pretty_assertions::assert_eq;

#[test]
fn test_parse_simple_command() {
    let mut parser = ShellParser::new();
    let cmd = parser.parse("ls -la");
    assert!(cmd.has_tree());
    assert!(!cmd.has_errors());
}

#[test]
fn test_extract_safe_commands() {
    let mut parser = ShellParser::new();
    let cmd = parser.parse("ls -la && pwd");
    let commands = cmd.try_extract_safe_commands().unwrap();
    assert_eq!(commands, vec![vec!["ls", "-la"], vec!["pwd"]]);
}

#[test]
fn test_extract_piped_commands() {
    let mut parser = ShellParser::new();
    let cmd = parser.parse("cat file | grep pattern | wc -l");
    let commands = cmd.try_extract_safe_commands().unwrap();
    assert_eq!(
        commands,
        vec![
            vec!["cat", "file"],
            vec!["grep", "pattern"],
            vec!["wc", "-l"]
        ]
    );
}

#[test]
fn test_reject_redirections() {
    let mut parser = ShellParser::new();
    let cmd = parser.parse("echo hi > output.txt");
    assert!(cmd.try_extract_safe_commands().is_none());
}

#[test]
fn test_reject_subshells() {
    let mut parser = ShellParser::new();
    let cmd = parser.parse("(ls && pwd)");
    assert!(cmd.try_extract_safe_commands().is_none());
}

#[test]
fn test_reject_command_substitution() {
    let mut parser = ShellParser::new();
    let cmd = parser.parse("echo $(pwd)");
    assert!(cmd.try_extract_safe_commands().is_none());
}

#[test]
fn test_reject_variable_expansion() {
    let mut parser = ShellParser::new();
    let cmd = parser.parse("echo $HOME");
    assert!(cmd.try_extract_safe_commands().is_none());
}

#[test]
fn test_extract_commands_unsafe() {
    let mut parser = ShellParser::new();
    let cmd = parser.parse("echo $HOME && ls");
    // extract_commands works even for unsafe commands
    let commands = cmd.extract_commands();
    assert_eq!(commands.len(), 2);
}

#[test]
fn test_parse_shell_invocation() {
    let mut parser = ShellParser::new();
    let argv = vec!["bash".to_string(), "-c".to_string(), "ls -la".to_string()];
    let cmd = parser.parse_shell_invocation(&argv).unwrap();
    let commands = cmd.try_extract_safe_commands().unwrap();
    assert_eq!(commands, vec![vec!["ls", "-la"]]);
}

#[test]
fn test_detect_shell_type() {
    assert_eq!(detect_shell_type(Path::new("/bin/bash")), ShellType::Bash);
    assert_eq!(detect_shell_type(Path::new("/usr/bin/zsh")), ShellType::Zsh);
    assert_eq!(detect_shell_type(Path::new("sh")), ShellType::Sh);
    assert_eq!(
        detect_shell_type(Path::new("powershell.exe")),
        ShellType::PowerShell
    );
    assert_eq!(detect_shell_type(Path::new("cmd.exe")), ShellType::Cmd);
}

#[test]
fn test_quoted_strings() {
    let mut parser = ShellParser::new();
    let cmd = parser.parse("echo 'hello world' \"foo bar\"");
    let commands = cmd.try_extract_safe_commands().unwrap();
    assert_eq!(commands, vec![vec!["echo", "hello world", "foo bar"]]);
}

#[test]
fn test_concatenated_args() {
    let mut parser = ShellParser::new();
    let cmd = parser.parse("rg -g\"*.py\" pattern");
    let commands = cmd.try_extract_safe_commands().unwrap();
    assert_eq!(commands, vec![vec!["rg", "-g*.py", "pattern"]]);
}
