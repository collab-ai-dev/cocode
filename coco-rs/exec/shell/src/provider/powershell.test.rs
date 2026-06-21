use super::*;
use crate::shell_types::Shell;
use crate::shell_types::get_shell;
use std::path::PathBuf;

fn powershell_shell() -> Option<Shell> {
    get_shell(ShellType::PowerShell, None)
}

#[tokio::test]
async fn build_exec_non_sandbox_emits_raw_ps_command() {
    let Some(shell) = powershell_shell() else {
        return;
    };
    let p = PowerShellProvider::from_shell(shell);
    let built = p
        .build_exec_command("Get-ChildItem", &BuildExecOpts::default())
        .await;
    assert!(built.command_string.starts_with("Get-ChildItem"));
    assert!(built.command_string.contains("Out-File"));
    assert!(built.command_string.contains("exit $_ec"));
}

#[tokio::test]
async fn build_exec_sandbox_emits_encoded_command() {
    let Some(shell) = powershell_shell() else {
        return;
    };
    let p = PowerShellProvider::from_shell(shell);
    let opts = BuildExecOpts {
        id: 7,
        sandbox_tmp_dir: Some(PathBuf::from("/tmp/sbx")),
        use_sandbox: true,
    };
    let built = p.build_exec_command("Get-ChildItem", &opts).await;
    assert!(built.command_string.contains("-EncodedCommand"));
    assert!(built.command_string.contains("-NoProfile"));
    assert!(built.cwd_file_path.starts_with("/tmp/sbx"));
}

#[tokio::test]
async fn spawn_args_includes_noprofile_noninteractive_command() {
    let Some(shell) = powershell_shell() else {
        return;
    };
    let p = PowerShellProvider::from_shell(shell);
    let args = p.spawn_args("Write-Output hi");
    assert_eq!(args[0], "-NoProfile");
    assert_eq!(args[1], "-NonInteractive");
    assert_eq!(args[2], "-Command");
    assert_eq!(args[3], "Write-Output hi");
}

#[tokio::test]
async fn ps_quote_escapes_single_quotes() {
    assert_eq!(PowerShellProvider::ps_quote("it's"), "'it''s'");
}

#[tokio::test]
async fn base64_roundtrip_decodes_to_utf16le() {
    let encoded = PowerShellProvider::encode_utf16le_base64("hi");
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&encoded)
        .unwrap();
    assert_eq!(bytes, [b'h', 0, b'i', 0]);
}

#[tokio::test]
async fn env_overrides_no_sandbox_empty() {
    let Some(shell) = powershell_shell() else {
        return;
    };
    let p = PowerShellProvider::from_shell(shell);
    let env = p
        .env_overrides("Get-ChildItem", &BuildExecOpts::default())
        .await;
    assert!(env.is_empty());
}
