use std::process::Stdio;

use pretty_assertions::assert_eq;

use super::HARDENED_CONFIG_ARGS;
use super::hardened_std_git;

#[test]
fn test_hardened_std_git_flags_precede_subcommand() {
    let mut cmd = hardened_std_git();
    cmd.arg("status");
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args,
        [
            "-c",
            "safe.bareRepository=explicit",
            "-c",
            "core.fsmonitor=false",
            "status",
        ]
    );
}

/// A bare repository reached by upward discovery must be rejected, while the
/// same invocation succeeds without the hardening flags — pins that
/// `safe.bareRepository=explicit` actually changes git's behavior.
#[test]
fn test_bare_repository_discovery_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bare = dir.path().join("planted.git");
    let init = std::process::Command::new("git")
        .args(["init", "--bare", "--quiet"])
        .arg(&bare)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let Ok(status) = init else {
        return; // No git on PATH — nothing to verify.
    };
    assert!(status.success());

    let unhardened = std::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(&bare)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run git");
    assert!(
        unhardened.success(),
        "bare repo is discoverable without flags"
    );

    let mut cmd = hardened_std_git();
    let hardened = cmd
        .args(["rev-parse", "--git-dir"])
        .current_dir(&bare)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run git");
    assert!(
        !hardened.success(),
        "hardened invocation must refuse discovery"
    );
}

#[test]
fn test_config_args_shape() {
    // `-c key=value` pairs: even length, alternating flag/value.
    assert_eq!(HARDENED_CONFIG_ARGS.len() % 2, 0);
    for pair in HARDENED_CONFIG_ARGS.chunks(2) {
        assert_eq!(pair[0], "-c");
        assert!(pair[1].contains('='));
    }
}
