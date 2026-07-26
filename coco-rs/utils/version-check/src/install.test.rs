use pretty_assertions::assert_eq;
use std::path::Path;

use super::InstallMethod;

#[test]
fn npm_global_installs_are_detected() {
    assert_eq!(
        InstallMethod::from_path(Path::new(
            "/usr/local/lib/node_modules/@cocode-cli/cocode-cli/vendor/coco"
        )),
        InstallMethod::Npm
    );
}

#[test]
fn pnpm_and_bun_win_over_the_node_modules_rule() {
    // Both keep their global store under a path that also contains
    // `node_modules`; matching npm first would print the wrong command.
    assert_eq!(
        InstallMethod::from_path(Path::new(
            "/home/dev/.local/share/pnpm/global/5/node_modules/@cocode-cli/cocode-cli/vendor/coco"
        )),
        InstallMethod::Pnpm
    );
    assert_eq!(
        InstallMethod::from_path(Path::new(
            "/home/dev/.bun/install/global/node_modules/@cocode-cli/cocode-cli/vendor/coco"
        )),
        InstallMethod::Bun
    );
}

#[test]
fn homebrew_and_cargo_layouts_are_detected() {
    assert_eq!(
        InstallMethod::from_path(Path::new("/opt/homebrew/bin/coco")),
        InstallMethod::Homebrew
    );
    assert_eq!(
        InstallMethod::from_path(Path::new("/usr/local/Cellar/cocode/0.1.1/bin/coco")),
        InstallMethod::Homebrew
    );
    assert_eq!(
        InstallMethod::from_path(Path::new("/home/dev/.cargo/bin/coco")),
        InstallMethod::Cargo
    );
}

#[test]
fn windows_paths_are_normalized() {
    assert_eq!(
        InstallMethod::from_path(Path::new(
            r"C:\Users\dev\AppData\Roaming\npm\node_modules\@cocode-cli\cocode-cli\vendor\coco.exe"
        )),
        InstallMethod::Npm
    );
}

#[test]
fn an_unrecognized_layout_offers_no_command() {
    let method = InstallMethod::from_path(Path::new("/home/dev/bin/coco"));
    assert_eq!(method, InstallMethod::Unknown);
    // The point of Unknown: never print an upgrade line that would fail.
    assert_eq!(method.upgrade_command(), None);
}

#[test]
fn every_known_method_offers_a_command() {
    for method in [
        InstallMethod::Npm,
        InstallMethod::Pnpm,
        InstallMethod::Bun,
        InstallMethod::Homebrew,
        InstallMethod::Cargo,
    ] {
        assert!(
            method.upgrade_command().is_some(),
            "{method:?} must know how to upgrade itself"
        );
    }
}
