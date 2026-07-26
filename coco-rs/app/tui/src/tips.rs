//! Startup tips — one line of capability discovery under the header.
//!
//! # Why these choices
//!
//! **Local, never fetched.** The obvious reference implementation pulls an
//! announcement file over the network at startup. coco does not: a tip is not
//! worth a startup request, a startup request to a vendor endpoint is a signal
//! coco should not be emitting on every launch, and a tip that can change
//! remotely is a channel for product copy rather than for teaching the tool.
//! The catalog ships in the binary and is reviewable in the diff.
//!
//! **Discovery, not marketing.** Every tip names something the user can do in
//! the build they are running — a key, a prefix, a command. That constrains the
//! catalog to things that must stay true, and the test below pins each tip's
//! text to a real keymap entry so a rebind or a removed feature fails CI
//! instead of shipping a lie.
//!
//! **One tip per day, not per launch.** Rotation is `days since epoch modulo
//! catalog length`. Restarting coco twenty times in an afternoon shows the same
//! tip twenty times, which is the point: a line that changes on every launch
//! reads as noise and gets filtered out by the second day. A daily line is a
//! drip feed, and it is deterministic, so snapshots and tests can pin it.
//!
//! **It never displaces content.** The tip is one dim line in the startup
//! header only, is skipped on terminals too narrow to hold it on one row, and
//! is off with `tui.tips = false`.

use crate::i18n::t;
use crate::keymap::KEYMAP;
use crate::keymap::displayed_combo;

/// Terminal width below which a tip is not worth showing: it would wrap and
/// push the header around to deliver an aside.
const MIN_TIP_WIDTH: u16 = 48;

/// One entry in the tip catalog.
///
/// An enum rather than a list of raw i18n keys so a tip cannot be referenced
/// that does not exist, and so [`Tip::text`] can attach the runtime data a tip
/// needs (a keymap-resolved combo) instead of hardcoding one into the string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Tip {
    /// `Shift+Enter` (or whatever this terminal can actually report) inserts a
    /// newline instead of submitting.
    Newline,
    /// `!` at the start of the composer runs a shell command.
    BashPrefix,
    /// `@` completes a path.
    FilePrefix,
    /// `#` writes a note to memory.
    MemoryPrefix,
    /// `Ctrl+O` opens the transcript reader.
    Transcript,
    /// `Ctrl+R` searches input history.
    HistorySearch,
    /// `Ctrl+G` opens the composer in `$EDITOR`.
    ExternalEditor,
    /// `Shift+Tab` cycles the permission mode.
    PermissionMode,
    /// `Ctrl+P` opens the command palette.
    CommandPalette,
    /// `Ctrl+B` sends the running command to the background.
    Background,
}

/// The catalog, in rotation order.
const TIPS: &[Tip] = &[
    Tip::Newline,
    Tip::BashPrefix,
    Tip::FilePrefix,
    Tip::Transcript,
    Tip::PermissionMode,
    Tip::HistorySearch,
    Tip::CommandPalette,
    Tip::ExternalEditor,
    Tip::MemoryPrefix,
    Tip::Background,
];

impl Tip {
    /// The keymap entry this tip talks about. Every tip has one — that is what
    /// keeps the catalog honest.
    fn keymap_id(self) -> &'static str {
        match self {
            Self::Newline => "input:newline",
            Self::BashPrefix => "prefix:bash",
            Self::FilePrefix => "prefix:file",
            Self::MemoryPrefix => "prefix:memory",
            Self::Transcript => "global:toggle_transcript",
            Self::HistorySearch => "global:history_search",
            Self::ExternalEditor => "input:external_editor",
            Self::PermissionMode => "global:cycle_permission_mode",
            Self::CommandPalette => "global:command_palette",
            Self::Background => "task:background",
        }
    }

    fn i18n_key(self) -> &'static str {
        match self {
            Self::Newline => "tip.newline",
            Self::BashPrefix => "tip.bash_prefix",
            Self::FilePrefix => "tip.file_prefix",
            Self::MemoryPrefix => "tip.memory_prefix",
            Self::Transcript => "tip.transcript",
            Self::HistorySearch => "tip.history_search",
            Self::ExternalEditor => "tip.external_editor",
            Self::PermissionMode => "tip.permission_mode",
            Self::CommandPalette => "tip.command_palette",
            Self::Background => "tip.background",
        }
    }

    /// Rendered text, with the combo resolved from the keymap at display time.
    ///
    /// Resolving rather than hardcoding is what makes the newline tip safe to
    /// print: on a terminal that cannot report Shift+Enter, the tip names the
    /// alternate that works.
    pub(crate) fn text(self) -> String {
        let combo = combo_for(self.keymap_id());
        t!(self.i18n_key(), combo = combo.as_str()).to_string()
    }
}

/// The displayed combo for a keymap id, or the id itself if the entry is gone.
///
/// A missing entry is a bug the test catches; falling back to the id keeps the
/// header renderable rather than panicking a user's session over a tip.
fn combo_for(keymap_id: &str) -> String {
    KEYMAP
        .iter()
        .find(|entry| entry.id == keymap_id)
        .map_or_else(
            || keymap_id.to_string(),
            |entry| displayed_combo(entry).to_string(),
        )
}

/// The tip for a given day, or `None` when tips are off or the terminal is too
/// narrow to hold one.
pub(crate) fn tip_for_day(enabled: bool, width: u16, days_since_epoch: i64) -> Option<Tip> {
    if !enabled || width < MIN_TIP_WIDTH || TIPS.is_empty() {
        return None;
    }
    // `rem_euclid` so a pre-epoch clock (a machine with a badly wrong date)
    // still lands inside the catalog instead of panicking on a negative index.
    let index = days_since_epoch.rem_euclid(TIPS.len() as i64) as usize;
    Some(TIPS[index])
}

/// Today's tip, from the system clock.
pub(crate) fn todays_tip(enabled: bool, width: u16) -> Option<Tip> {
    tip_for_day(enabled, width, days_since_epoch(chrono::Utc::now()))
}

fn days_since_epoch(now: chrono::DateTime<chrono::Utc>) -> i64 {
    now.date_naive()
        .signed_duration_since(chrono::NaiveDate::default())
        .num_days()
}

#[cfg(test)]
#[path = "tips.test.rs"]
mod tests;
