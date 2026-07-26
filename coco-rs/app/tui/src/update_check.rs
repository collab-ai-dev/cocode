//! Background "a newer coco exists" check and how it reaches the user.
//!
//! Two surfaces, deliberately, because they answer different questions:
//!
//! - A **toast** when the answer arrives, which is what makes the check worth
//!   running at all. It is transient and interruptible; a modal here would stop
//!   a user mid-thought to deliver news that keeps.
//! - An **ambient status item** that stays for the session, so someone who
//!   looked away while the toast was up can still find the version and the
//!   command. It is the lowest status-bar priority, so a narrow terminal drops
//!   it before anything load-bearing.
//!
//! The check itself is an isolated stream, in the shape the other optional TUI
//! subsystems use: a task with its own channel, folded into UI state here and
//! never bridged into `CoreEvent`.

use std::time::Duration;

use coco_utils_version_check::UpgradeNotice;
use tokio::sync::mpsc;

/// How long the arrival toast stays up. Longer than an ordinary info toast:
/// the message carries a command the user may want to copy, and it competes
/// with whatever they were already reading.
const TOAST_DURATION: Duration = Duration::from_secs(12);

/// Spawn the check. Returns the receiver the run loop selects on, or `None`
/// when there is nothing to check for — `tui.update_check` is off, this is a
/// source build, or the cache is fresh and holds no upgrade.
///
/// The cached answer is consulted *synchronously* first and, when it already
/// names a newer version, delivered immediately — startup never waits on the
/// network for news that is already on disk.
pub(crate) fn spawn(
    enabled: bool,
    current_version: &'static str,
) -> Option<mpsc::Receiver<UpgradeNotice>> {
    // Checked before anything else touches the disk or the network: `false`
    // means cocode makes no outbound request on its own behalf, and reads no
    // cache to decide that.
    if !enabled {
        return None;
    }
    // The caller is a public constructor; a caller outside a runtime would
    // otherwise take a `tokio::spawn` panic at startup for the sake of a
    // version banner. Nothing about this feature is worth that.
    if tokio::runtime::Handle::try_current().is_err() {
        return None;
    }
    let path = coco_utils_version_check::default_cache_path();
    let cache = coco_utils_version_check::read_cache(&path);
    let cached_notice = cache
        .as_ref()
        .and_then(|cache| coco_utils_version_check::notice_from_cache(cache, current_version));
    let now = chrono::Utc::now();
    let refresh_due =
        coco_utils_version_check::should_refresh(cache.as_ref(), current_version, now);

    if cached_notice.is_none() && !refresh_due {
        return None;
    }

    let (tx, rx) = mpsc::channel(1);
    tokio::spawn(async move {
        if let Some(notice) = cached_notice {
            // Fire the cached answer first: a refresh that hangs must not
            // swallow news that was already on disk.
            let _ = tx.send(notice).await;
        }
        if !refresh_due {
            return;
        }
        match coco_utils_version_check::refresh_cache(&path, now).await {
            Ok(cache) => {
                if let Some(notice) =
                    coco_utils_version_check::notice_from_cache(&cache, current_version)
                {
                    let _ = tx.send(notice).await;
                }
            }
            // A registry that is down, blocked, or slow is the expected case on
            // plenty of networks, and it is not the user's problem to see.
            Err(err) => tracing::debug!(
                target: "coco_tui::update_check",
                error = %err,
                "update check failed",
            ),
        }
    });
    Some(rx)
}

/// The toast text: what is available, and what to run.
pub(crate) fn toast_message(notice: &UpgradeNotice) -> String {
    match notice.upgrade_command.as_deref() {
        Some(command) => crate::i18n::t!(
            "toast.update_available_command",
            version = notice.latest_version.as_str(),
            command = command,
        )
        .to_string(),
        // Install method unknown: say what is available without inventing a
        // command that would fail.
        None => crate::i18n::t!(
            "toast.update_available",
            version = notice.latest_version.as_str(),
        )
        .to_string(),
    }
}

/// Compact status-bar label, e.g. `↑ 0.2.0`.
pub(crate) fn status_label(notice: &UpgradeNotice) -> String {
    format!("↑ {}", notice.latest_version)
}

/// Fold an arrived notice into UI state. Returns whether a redraw is needed.
pub(crate) fn apply(state: &mut crate::state::AppState, notice: UpgradeNotice) -> bool {
    tracing::info!(
        target: "coco_tui::update_check",
        current = %notice.current_version,
        latest = %notice.latest_version,
        "newer coco available",
    );
    state.ui.add_toast(crate::state::ui::Toast {
        message: toast_message(&notice),
        severity: crate::state::ui::ToastSeverity::Info,
        created_at: std::time::Instant::now(),
        duration: TOAST_DURATION,
    });
    state.ui.upgrade_notice = Some(notice);
    true
}

#[cfg(test)]
#[path = "update_check.test.rs"]
mod tests;
