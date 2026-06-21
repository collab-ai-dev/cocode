use super::*;

#[test]
fn test_excluded_exports_regex() {
    let regex = excluded_exports_regex();
    assert!(regex.contains("PWD"));
    assert!(regex.contains("OLDPWD"));
    assert!(regex.contains("|"));
}

#[test]
fn test_zsh_script_contains_marker() {
    let script = zsh_snapshot_script();
    assert!(script.contains("# Snapshot file"));
    assert!(script.contains("unalias -a"));
    assert!(
        script.contains("typeset +f"),
        "should list function names for filtering"
    );
    assert!(
        script.contains("grep -vE '^_[^_]'"),
        "should filter single-underscore completion functions"
    );
    assert!(script.contains("setopt"));
    assert!(script.contains("alias -L"));
    assert!(script.contains("export -p"));
    assert!(script.contains("head -n 1000"));
}

#[test]
fn test_bash_script_contains_marker() {
    let script = bash_snapshot_script();
    assert!(script.contains("# Snapshot file"));
    assert!(script.contains("unalias -a"));
    assert!(
        script.contains("base64"),
        "should use base64 encoding for function capture"
    );
    assert!(
        script.contains("grep -vE '^_[^_]'"),
        "should filter single-underscore completion functions"
    );
    assert!(
        script.contains("shopt -p"),
        "should capture all shopt options"
    );
    assert!(
        script.contains("shopt -s expand_aliases"),
        "should enable alias expansion"
    );
    assert!(
        script.contains("alias -- "),
        "should use -- prefix for alias names"
    );
    assert!(script.contains("export -p"));
    assert!(script.contains("head -n 1000"));
}

#[test]
fn test_sh_script_contains_marker() {
    let script = sh_snapshot_script();
    assert!(script.contains("# Snapshot file"));
    assert!(script.contains("unalias -a"));
    assert!(script.contains("export -p"));
    assert!(script.contains("typeset -f"));
    assert!(script.contains("declare -f"));
}

#[test]
fn test_powershell_script_contains_marker() {
    let script = powershell_snapshot_script();
    assert!(script.contains("# Snapshot file"));
    assert!(script.contains("Remove-Item Alias:*"));
    assert!(script.contains("Get-ChildItem Function:"));
    assert!(script.contains("Get-Alias"));
    assert!(script.contains("Get-ChildItem Env:"));
}

#[test]
fn test_scripts_filter_excluded_vars() {
    let zsh = zsh_snapshot_script();
    let bash = bash_snapshot_script();
    let sh = sh_snapshot_script();

    for script in [&zsh, &bash, &sh] {
        // The EXCLUDED_EXPORTS placeholder should have been replaced
        assert!(
            !script.contains("EXCLUDED_EXPORTS"),
            "placeholder should be replaced with actual var names"
        );
        assert!(
            script.contains("PWD|OLDPWD"),
            "should contain expanded exclusion pattern"
        );
    }
}

#[cfg(unix)]
#[test]
fn test_bash_snapshot_filters_invalid_exports() {
    use std::process::Command;

    let output = Command::new("/bin/bash")
        .arg("-c")
        .arg(bash_snapshot_script())
        .env("BASH_ENV", "/dev/null")
        .env("VALID_NAME", "ok")
        .env("PWD", "/tmp/stale")
        .env("BAD-NAME", "broken")
        .output()
        .expect("run bash");

    assert!(output.status.success(), "bash script should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("VALID_NAME"),
        "should include valid exports"
    );
    assert!(
        !stdout.contains("PWD=/tmp/stale"),
        "should exclude PWD from exports"
    );
    assert!(
        !stdout.contains("BAD-NAME"),
        "should exclude invalid variable names"
    );
}
