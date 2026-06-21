use super::extract_powershell_command;

#[test]
fn extracts_basic_powershell_command() {
    let cmd = vec![
        "powershell".to_string(),
        "-Command".to_string(),
        "Write-Host hi".to_string(),
    ];
    let (_shell, script) = extract_powershell_command(&cmd).expect("extract");
    assert_eq!(script, "Write-Host hi");
}

#[test]
fn extracts_lowercase_flags() {
    let cmd = vec![
        "powershell".to_string(),
        "-nologo".to_string(),
        "-command".to_string(),
        "Write-Host hi".to_string(),
    ];
    let (_shell, script) = extract_powershell_command(&cmd).expect("extract");
    assert_eq!(script, "Write-Host hi");
}

#[test]
fn extracts_with_noprofile_and_alias() {
    let cmd = vec![
        "pwsh".to_string(),
        "-NoProfile".to_string(),
        "-c".to_string(),
        "Get-ChildItem | Select-String foo".to_string(),
    ];
    let (_shell, script) = extract_powershell_command(&cmd).expect("extract");
    assert_eq!(script, "Get-ChildItem | Select-String foo");
}
