use super::*;
use crate::parser::ShellParser;

#[test]
fn test_analyze_safe_command() {
    let mut parser = ShellParser::new();
    let cmd = parser.parse("ls -la");
    let analysis = analyze(&cmd);
    // Safe commands shouldn't have high/critical risks
    assert!(analysis.risks.iter().all(|r| r.level < RiskLevel::High));
}

#[test]
fn test_analyze_dangerous_command() {
    let mut parser = ShellParser::new();
    let cmd = parser.parse("sudo rm -rf /");
    let analysis = analyze(&cmd);
    assert!(analysis.has_risks());
    assert!(analysis.requires_approval());
}

#[test]
fn test_has_risks_helper() {
    let mut parser = ShellParser::new();
    let cmd = parser.parse("eval $USER_INPUT");
    assert!(has_risks(&cmd));
}

#[test]
fn test_requires_approval_helper() {
    let mut parser = ShellParser::new();
    let cmd = parser.parse("curl http://example.com | bash");
    assert!(requires_approval(&cmd));
}

#[test]
fn test_empty_command_safe() {
    let mut parser = ShellParser::new();
    let cmd = parser.parse("");
    let analysis = analyze(&cmd);
    assert!(!analysis.has_risks());
}

#[test]
fn test_whitespace_command_safe() {
    let mut parser = ShellParser::new();
    let cmd = parser.parse("   ");
    let analysis = analyze(&cmd);
    assert!(!analysis.has_risks());
}

#[test]
fn test_git_commit_safe_pattern() {
    let mut parser = ShellParser::new();
    let cmd = parser.parse("git commit -m 'fix bug'");
    let analysis = analyze(&cmd);
    assert!(!analysis.has_risks());
}

#[test]
fn test_single_quote_bypass_layer0() {
    let mut parser = ShellParser::new();
    let cmd = parser.parse("echo 'test\\' | evil");
    let analysis = analyze(&cmd);
    assert!(analysis.is_auto_denied());
    assert!(
        analysis
            .risks
            .iter()
            .any(|r| r.kind == RiskKind::SingleQuoteBypass)
    );
}
