//! Vim mode state machine.
//!
//! Complete vi input handling: normal/insert modes, motions, operators,
//! text objects, counts, find, and dot-repeat. All positions are UTF-8
//! byte offsets into `TextArea`; the state machine operates directly on
//! `TextArea` via `wiring::dispatch_vim_key`.

mod motions;
mod operators;
mod text_objects;
mod transitions;
pub mod wiring;

/// Vim operator type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    Delete,
    Change,
    Yank,
}

/// Find motion type (f/F/t/T).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindType {
    /// Find forward to char (inclusive).
    F,
    /// Find backward to char (inclusive).
    BigF,
    /// Find forward till char (exclusive).
    T,
    /// Find backward till char (exclusive).
    BigT,
}

/// Text object scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextObjScope {
    Inner,
    Around,
}

/// Complete vim state.
#[derive(Debug, Clone)]
pub enum VimState {
    /// Insert mode — keys go directly to input.
    Insert { inserted_text: String },
    /// Normal mode — keys are parsed as commands.
    Normal { command: CommandState },
}

/// Command state machine for normal mode.
///
/// All `count` fields are `usize` — vim counts are non-negative integers
/// that index into text and bound iteration loops, so the type lines up
/// with everything they multiply against.
#[derive(Debug, Clone)]
pub enum CommandState {
    /// Waiting for a command key.
    Idle,
    /// Accumulating count digits (e.g., "3" before a motion).
    Count { digits: String },
    /// Operator entered (d/c/y), waiting for motion or text object.
    OperatorPending { op: Operator, count: usize },
    /// Operator + count digits (e.g., "d3" waiting for motion).
    OperatorCount {
        op: Operator,
        count: usize,
        digits: String,
    },
    /// Operator + find type, waiting for character.
    OperatorFind {
        op: Operator,
        count: usize,
        find: FindType,
    },
    /// Operator + text object scope (i/a), waiting for object key.
    OperatorTextObj {
        op: Operator,
        count: usize,
        scope: TextObjScope,
    },
    /// Find motion (f/F/t/T), waiting for character.
    Find { find: FindType, count: usize },
    /// Waiting for second key after 'g'.
    G { count: usize },
    /// Operator + g, waiting for second key.
    OperatorG { op: Operator, count: usize },
    /// Replace mode (r), waiting for replacement character.
    Replace { count: usize },
    /// Indent (> or <), waiting for motion.
    Indent { dir: IndentDir, count: usize },
}

/// Indent direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndentDir {
    Right,
    Left,
}

/// Persistent state that survives across commands (memory for repeats/pastes).
#[derive(Debug, Clone, Default)]
pub struct PersistentState {
    /// Last change for dot-repeat.
    pub last_change: Option<RecordedChange>,
    /// Last find motion for ; and , repeat.
    pub last_find: Option<(FindType, char)>,
    /// Yank register content.
    pub register: String,
    /// Whether register content is linewise (governs `p`/`P` behavior).
    pub register_is_linewise: bool,
}

/// Recorded change for dot-repeat (`.`).
///
/// Captured at the end of a successful mutating command so `.` can replay
/// it against the current cursor. Insert-mode sessions are NOT recorded
/// yet — they require tracking keystrokes across the whole session and
/// aren't part of the V1 dot-repeat surface.
#[derive(Debug, Clone)]
pub enum RecordedChange {
    /// Operator + motion (e.g. `dw`, `c2w`, `y$`).
    OperatorMotion {
        op: Operator,
        motion: char,
        count: usize,
    },
    /// Doubled operator (`dd`, `cc`, `yy`).
    OperatorLine { op: Operator, count: usize },
    /// `x` — delete character under cursor, with count for `Nx`.
    DeleteChar { count: usize },
    /// `r<ch>` — replace character under cursor.
    ReplaceChar { ch: char, count: usize },
    /// `p` / `P` — paste from the register.
    Put { before: bool },
}

impl VimState {
    /// Create initial state (normal mode, idle).
    pub fn new() -> Self {
        VimState::Normal {
            command: CommandState::Idle,
        }
    }

    /// Whether currently in insert mode.
    pub fn is_insert(&self) -> bool {
        matches!(self, VimState::Insert { .. })
    }

    /// Whether currently in normal mode.
    pub fn is_normal(&self) -> bool {
        matches!(self, VimState::Normal { .. })
    }

    /// Enter insert mode.
    pub fn enter_insert(&mut self) {
        *self = VimState::Insert {
            inserted_text: String::new(),
        };
    }

    /// Enter normal mode, resetting command state.
    pub fn enter_normal(&mut self) {
        *self = VimState::Normal {
            command: CommandState::Idle,
        };
    }

    /// Get mode label for status bar.
    pub fn mode_label(&self) -> &str {
        match self {
            VimState::Insert { .. } => "INSERT",
            VimState::Normal {
                command: CommandState::Idle,
            } => "NORMAL",
            VimState::Normal { .. } => "NORMAL (pending)",
        }
    }
}

impl Default for VimState {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandState {
    /// Reset to idle.
    pub fn reset(&mut self) {
        *self = CommandState::Idle;
    }
}

/// Bundled vim mode + persistent register state.
///
/// Single ownership site so `InputState` doesn't have to hold the two
/// fields side-by-side. `state` flips between Normal and Insert; `persistent`
/// keeps yank-register / last-find / dot-repeat memory across commands.
///
/// `enabled` gates whether the InsertChar / Cancel dispatch consults the
/// state machine. Defaults to `false` — typing then inserts characters
/// directly and Esc routes to the standard Cancel flow, matching what a
/// non-vim user expects. The `/vim` slash command persists user intent
/// to `~/.coco/state/editor_mode`; wiring that file back into this flag
/// at TUI startup is tracked separately.
#[derive(Debug, Clone, Default)]
pub struct VimRuntime {
    pub state: VimState,
    pub persistent: PersistentState,
    pub enabled: bool,
}

impl VimRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_normal(&self) -> bool {
        self.state.is_normal()
    }

    pub fn is_insert(&self) -> bool {
        self.state.is_insert()
    }

    /// True when vim is on AND currently in Normal mode — the gate the
    /// InsertChar dispatcher uses to decide whether to route printable
    /// keys through the state machine.
    pub fn normal_dispatch_active(&self) -> bool {
        self.enabled && self.is_normal()
    }

    /// True when vim is on AND currently in Insert mode — the gate the
    /// Esc/Cancel dispatcher uses to decide whether to transition back
    /// to Normal mode instead of running the standard Cancel flow.
    pub fn insert_escape_active(&self) -> bool {
        self.enabled && self.is_insert()
    }
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
