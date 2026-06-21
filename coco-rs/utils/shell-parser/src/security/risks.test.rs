use super::*;

#[test]
fn test_risk_level_ordering() {
    assert!(RiskLevel::Low < RiskLevel::Medium);
    assert!(RiskLevel::Medium < RiskLevel::High);
    assert!(RiskLevel::High < RiskLevel::Critical);
}

#[test]
fn test_security_risk_creation() {
    let risk = SecurityRisk::new(RiskKind::CodeExecution, "eval detected");
    assert_eq!(risk.kind, RiskKind::CodeExecution);
    assert_eq!(risk.level, RiskLevel::Critical);
    assert_eq!(risk.phase, RiskPhase::Ask);
}

#[test]
fn test_security_analysis() {
    let mut analysis = SecurityAnalysis::new();
    assert!(!analysis.has_risks());

    analysis.add_risk(SecurityRisk::new(RiskKind::ObfuscatedFlags, "test"));
    assert!(analysis.has_risks());
    assert_eq!(analysis.max_level, Some(RiskLevel::Medium));

    analysis.add_risk(SecurityRisk::new(RiskKind::CodeExecution, "test2"));
    assert_eq!(analysis.max_level, Some(RiskLevel::Critical));
}

#[test]
fn test_unsafe_heredoc_substitution() {
    let risk = SecurityRisk::new(RiskKind::UnsafeHeredocSubstitution, "test heredoc risk");
    assert_eq!(risk.level, RiskLevel::Medium);
    assert_eq!(risk.phase, RiskPhase::Ask);
    assert_eq!(risk.kind.name(), "unsafe heredoc substitution");
}

#[test]
fn test_requires_approval() {
    let mut analysis = SecurityAnalysis::new();
    analysis.add_risk(SecurityRisk::new(RiskKind::ObfuscatedFlags, "test"));
    assert!(!analysis.requires_approval()); // Deny phase, not Ask

    analysis.add_risk(SecurityRisk::new(RiskKind::CodeExecution, "test2"));
    assert!(analysis.requires_approval()); // Ask phase
}
