//! Canonical DEC private-mode / stateful-protocol teardown ledger.
//!
//! Every terminal mode coco can leave enabled has exactly one documented
//! teardown sequence here, with the ordering invariants encoded as tests rather
//! than scattered across comments at the four teardown sites (`leave_tui_modes`,
//! `restore_terminal`, `Tui::drop`, `keyboard_modes`).
//!
//! [`RESTORE_SEQ`] is the raw-bytes form for the async-signal-safe fault handler
//! (workstream A5): a signal handler must not call crossterm (it allocates and
//! is not async-signal-safe), so it `write(2)`s these precomputed bytes straight
//! to the tty fd.
//!
//! ## Ordering invariants (encoded as tests, not just prose)
//!
//! 1. **`?2026l` (end synchronized update) leads.** coco paints every frame
//!    inside a BSU/ESU window, so a fault mid-frame can leave the terminal stuck
//!    inside an *open* synchronized-update window — the screen freezes and reads
//!    as a total lockup. A relaying multiplexer must see the ESU before anything
//!    else, or the entire restore is buffered/withheld behind the open window.
//! 2. **kitty pop (`CSI < u`) precedes any `?1049l`.** kitty keeps a per-screen
//!    keyboard-mode stack, so the flags must be reset on the screen that pushed
//!    them, before leaving the alternate screen. `?1049l` is *conditional* —
//!    coco's main surface never enters alt-screen — so it stays OUTSIDE this
//!    constant; issuing it unconditionally would corrupt a terminal that never
//!    left the main buffer.
//! 3. **`?25h` (show cursor) is last** so the cursor is visible after restore.

/// The individual restore sequences, in canonical teardown order. Named so the
/// ledger tests can assert both membership and ordering, and so [`RESTORE_SEQ`]
/// is verifiably their concatenation.
pub mod seq {
    /// End synchronized update (DECRST 2026). MUST be emitted first.
    pub const END_SYNC_UPDATE: &[u8] = b"\x1b[?2026l";
    /// Reset every pushed kitty keyboard-enhancement level (`CSI < u`).
    pub const KITTY_POP_ALL: &[u8] = b"\x1b[<u";
    /// Disable bracketed paste (DECRST 2004).
    pub const DISABLE_BRACKETED_PASTE: &[u8] = b"\x1b[?2004l";
    /// Disable focus-change reporting (DECRST 1004).
    pub const DISABLE_FOCUS_REPORTING: &[u8] = b"\x1b[?1004l";
    /// Disable alternate-scroll (DECRST 1007).
    pub const DISABLE_ALTERNATE_SCROLL: &[u8] = b"\x1b[?1007l";
    /// Disable xterm modifyOtherKeys mode used inside compatible tmux sessions.
    pub const DISABLE_MODIFY_OTHER_KEYS: &[u8] = b"\x1b[>4;0m";
    /// Show the cursor (DECSET 25). MUST be emitted last.
    pub const SHOW_CURSOR: &[u8] = b"\x1b[?25h";

    /// The sequences in canonical teardown order — the ordered ledger.
    pub const ORDERED: &[&[u8]] = &[
        END_SYNC_UPDATE,
        KITTY_POP_ALL,
        DISABLE_BRACKETED_PASTE,
        DISABLE_FOCUS_REPORTING,
        DISABLE_ALTERNATE_SCROLL,
        DISABLE_MODIFY_OTHER_KEYS,
        SHOW_CURSOR,
    ];
}

/// Concatenation of every unconditional restore sequence in canonical teardown
/// order. This is the byte blob the A5 signal handler writes verbatim.
///
/// `?1049l` (leave alternate screen) is intentionally excluded: it is
/// conditional (coco's main surface never enters alt-screen) and issuing it
/// unconditionally would corrupt a terminal still on the main buffer.
pub const RESTORE_SEQ: &[u8] =
    b"\x1b[?2026l\x1b[<u\x1b[?2004l\x1b[?1004l\x1b[?1007l\x1b[>4;0m\x1b[?25h";

/// Canonical prefix that must be emitted before a conditional alt-screen
/// leave: end synchronized update, then reset the kitty keyboard stack on the
/// screen that owns it.
pub const RESTORE_PREFIX: &[u8] = b"\x1b[?2026l\x1b[<u";

/// Canonical suffix emitted after any conditional alt-screen leave.
pub const RESTORE_SUFFIX: &[u8] = b"\x1b[?2004l\x1b[?1004l\x1b[?1007l\x1b[>4;0m\x1b[?25h";

#[cfg(test)]
#[path = "restore_seq.test.rs"]
mod tests;
