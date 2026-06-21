use super::*;

#[test]
fn test_parse_installer_type_rustup() {
    assert_eq!(
        LspInstaller::parse_installer_type("rustup component add rust-analyzer"),
        InstallerType::Rustup
    );
}

#[test]
fn test_parse_installer_type_go() {
    assert_eq!(
        LspInstaller::parse_installer_type("go install golang.org/x/tools/gopls@latest"),
        InstallerType::Go
    );
}

#[test]
fn test_parse_installer_type_npm() {
    assert_eq!(
        LspInstaller::parse_installer_type("npm install -g pyright"),
        InstallerType::Npm
    );
    assert_eq!(
        LspInstaller::parse_installer_type("npm install -g typescript-language-server typescript"),
        InstallerType::Npm
    );
}

#[test]
fn test_parse_installer_type_unknown() {
    assert_eq!(
        LspInstaller::parse_installer_type("brew install something"),
        InstallerType::Unknown
    );
    assert_eq!(
        LspInstaller::parse_installer_type("apt-get install lsp"),
        InstallerType::Unknown
    );
}

#[test]
fn test_installer_type_display() {
    assert_eq!(format!("{}", InstallerType::Rustup), "rustup");
    assert_eq!(format!("{}", InstallerType::Go), "go");
    assert_eq!(format!("{}", InstallerType::Npm), "npm");
    assert_eq!(format!("{}", InstallerType::Unknown), "shell");
}

#[tokio::test]
async fn test_is_installed_unknown_server() {
    // Unknown server should return false
    assert!(!LspInstaller::is_installed("unknown-server").await);
}
