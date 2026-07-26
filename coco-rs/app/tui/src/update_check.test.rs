use coco_utils_version_check::UpgradeNotice;
use pretty_assertions::assert_eq;

use super::apply;
use super::status_label;
use super::toast_message;
use crate::i18n::locale_test_guard;
use crate::state::AppState;

fn notice(command: Option<&str>) -> UpgradeNotice {
    UpgradeNotice {
        current_version: "0.1.1".to_string(),
        latest_version: "0.2.0".to_string(),
        upgrade_command: command.map(str::to_string),
    }
}

#[test]
fn the_toast_carries_the_command_for_this_installation() {
    let _locale = locale_test_guard("en");
    let message = toast_message(&notice(Some(
        "npm install -g @cocode-cli/cocode-cli@latest",
    )));
    assert!(message.contains("0.2.0"), "{message}");
    assert!(message.contains("npm install -g"), "{message}");
}

#[test]
fn an_unknown_install_method_announces_without_a_command() {
    let _locale = locale_test_guard("en");
    let message = toast_message(&notice(None));
    assert!(message.contains("0.2.0"), "{message}");
    // Inventing a command that fails is worse than offering none.
    assert!(!message.contains("install"), "{message}");
}

#[test]
fn the_status_label_is_compact_enough_for_the_bar() {
    assert_eq!(status_label(&notice(None)), "↑ 0.2.0");
}

#[test]
fn applying_a_notice_toasts_once_and_persists_it_for_the_status_bar() {
    let _locale = locale_test_guard("en");
    let mut state = AppState::default();
    assert!(apply(&mut state, notice(Some("brew upgrade cocode"))));
    assert_eq!(state.ui.toasts.len(), 1);
    // The toast expires; the status item is what remains for the session.
    assert_eq!(
        state
            .ui
            .upgrade_notice
            .as_ref()
            .map(|n| n.latest_version.as_str()),
        Some("0.2.0"),
    );
}

#[test]
fn spawning_outside_a_tokio_runtime_is_a_no_op_not_a_panic() {
    // The caller is a public constructor; a caller without a runtime must lose
    // the banner, not the session.
    assert!(super::spawn(/*enabled*/ true, "0.1.1").is_none());
}

#[tokio::test]
async fn a_disabled_check_makes_no_request_and_reads_no_cache() {
    // `tui.update_check = false` has to mean *nothing happens* — not "check
    // anyway and stay quiet about it".
    assert!(super::spawn(/*enabled*/ false, "0.1.1").is_none());
}
