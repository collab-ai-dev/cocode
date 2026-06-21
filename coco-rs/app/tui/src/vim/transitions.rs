//! Vim state transitions — process key input and update state.
//!
//! All positions are byte offsets into `TextArea::text()`. Motion functions
//! return new cursor targets; transitions build the deletion ranges by
//! consulting `is_inclusive_motion` so operator+motion semantics match real
//! vim (e.g. `dw` deletes through the start of the next word exclusive,
//! while `de` deletes through the end-of-word inclusive).

use std::ops::Range;

use super::CommandState;
use super::FindType;
use super::Operator;
use super::PersistentState;
use super::TextObjScope;
use super::motions;
use super::motions::next_char_boundary;
use super::motions::prev_char_boundary;
use super::operators;
use super::text_objects;
use coco_tui_ui::widgets::TextArea;

/// Result of processing a key in vim mode.
pub enum VimAction {
    /// Key was handled, no further action needed.
    Handled,
    /// Should enter insert mode at current position.
    EnterInsert,
    /// Should enter insert mode at end of line.
    EnterInsertEnd,
    /// Should enter insert mode after cursor.
    EnterInsertAfter,
    /// Should enter insert mode at line start.
    EnterInsertHome,
    /// Open line below and enter insert.
    OpenBelow,
    /// Open line above and enter insert.
    OpenAbove,
    /// Key not handled — pass to default handler.
    Unhandled,
    /// Submit input (Enter in normal mode).
    Submit,
}

/// Process a key in normal mode.
pub fn process_normal_key(
    ch: char,
    textarea: &mut TextArea,
    command: &mut CommandState,
    persistent: &mut PersistentState,
) -> VimAction {
    match command {
        CommandState::Idle => process_idle_key(ch, textarea, command, persistent),
        CommandState::Count { digits } => {
            if ch.is_ascii_digit() {
                digits.push(ch);
                VimAction::Handled
            } else {
                let count = digits.parse::<usize>().unwrap_or(1).max(1);
                *command = CommandState::Idle;
                process_motion_with_count(ch, textarea, count, command, persistent)
            }
        }
        CommandState::OperatorPending { op, count } => {
            let op = *op;
            let count = *count;
            process_operator_motion(ch, textarea, op, count, command, persistent)
        }
        CommandState::OperatorCount { op, count, digits } => {
            if ch.is_ascii_digit() {
                digits.push(ch);
                VimAction::Handled
            } else {
                let inner = digits.parse::<usize>().unwrap_or(1).max(1);
                let total = (*count).saturating_mul(inner);
                let op = *op;
                *command = CommandState::Idle;
                process_operator_motion(ch, textarea, op, total, command, persistent)
            }
        }
        CommandState::Find { find, count: _ } => {
            let find = *find;
            execute_find(ch, textarea, find, persistent);
            command.reset();
            VimAction::Handled
        }
        CommandState::OperatorFind { op, count: _, find } => {
            let op = *op;
            let find = *find;
            if let Some(range) = find_operator_range(textarea, find, ch) {
                operators::apply_operator(textarea, op, range, persistent);
            }
            command.reset();
            if op == Operator::Change {
                VimAction::EnterInsert
            } else {
                VimAction::Handled
            }
        }
        CommandState::OperatorTextObj {
            op,
            count: _,
            scope,
        } => {
            let op = *op;
            let scope = *scope;
            if let Some(range) = resolve_text_object(ch, textarea.text(), textarea.cursor(), scope)
            {
                operators::apply_operator(textarea, op, range, persistent);
            }
            command.reset();
            if op == Operator::Change {
                VimAction::EnterInsert
            } else {
                VimAction::Handled
            }
        }
        CommandState::G { count: _ } => {
            if ch == 'g' {
                textarea.set_cursor(motions::go_to_top(textarea.text()));
            }
            command.reset();
            VimAction::Handled
        }
        CommandState::OperatorG { op, count: _ } => {
            let op = *op;
            if ch == 'g' {
                let cursor = textarea.cursor();
                let target = motions::go_to_top(textarea.text());
                let range = target.min(cursor)..cursor.max(target);
                operators::apply_operator(textarea, op, range, persistent);
            }
            command.reset();
            if op == Operator::Change {
                VimAction::EnterInsert
            } else {
                VimAction::Handled
            }
        }
        CommandState::Replace { count } => {
            let count = *count;
            operators::replace_char(textarea, ch);
            persistent.last_change = Some(super::RecordedChange::ReplaceChar { ch, count });
            command.reset();
            VimAction::Handled
        }
        CommandState::Indent { dir: _, count: _ } => {
            // Indent is a no-op for the single-line composer.
            command.reset();
            VimAction::Handled
        }
    }
}

fn process_idle_key(
    ch: char,
    textarea: &mut TextArea,
    command: &mut CommandState,
    persistent: &mut PersistentState,
) -> VimAction {
    match ch {
        // Mode switches
        'i' => VimAction::EnterInsert,
        'I' => VimAction::EnterInsertHome,
        'a' => VimAction::EnterInsertAfter,
        'A' => VimAction::EnterInsertEnd,
        'o' => VimAction::OpenBelow,
        'O' => VimAction::OpenAbove,

        // Motions
        'h' => {
            textarea.move_cursor_left();
            VimAction::Handled
        }
        'l' => {
            textarea.move_cursor_right();
            VimAction::Handled
        }
        'w' => {
            let new = motions::word_forward(textarea.text(), textarea.cursor());
            textarea.set_cursor(new);
            VimAction::Handled
        }
        'b' => {
            let new = motions::word_backward(textarea.text(), textarea.cursor());
            textarea.set_cursor(new);
            VimAction::Handled
        }
        'e' => {
            let new = motions::word_end(textarea.text(), textarea.cursor());
            textarea.set_cursor(new);
            VimAction::Handled
        }
        '0' => {
            let new = motions::line_start(textarea.text(), textarea.cursor());
            textarea.set_cursor(new);
            VimAction::Handled
        }
        '^' => {
            let new = motions::first_non_blank(textarea.text(), textarea.cursor());
            textarea.set_cursor(new);
            VimAction::Handled
        }
        '$' => {
            let new = motions::line_end(textarea.text(), textarea.cursor());
            textarea.set_cursor(new);
            VimAction::Handled
        }
        'G' => {
            let new = motions::go_to_bottom(textarea.text());
            textarea.set_cursor(new);
            VimAction::Handled
        }

        // Operators
        'd' => {
            *command = CommandState::OperatorPending {
                op: Operator::Delete,
                count: 1,
            };
            VimAction::Handled
        }
        'c' => {
            *command = CommandState::OperatorPending {
                op: Operator::Change,
                count: 1,
            };
            VimAction::Handled
        }
        'y' => {
            *command = CommandState::OperatorPending {
                op: Operator::Yank,
                count: 1,
            };
            VimAction::Handled
        }

        // Single-key operations
        'x' => {
            // `x` deletes char under cursor; also stash it into the register
            // so a subsequent `p` pastes it (matches vim's small-delete rule).
            let cursor = textarea.cursor();
            let end = next_char_boundary(textarea.text(), cursor);
            if end > cursor {
                let ch = textarea.text()[cursor..end].to_string();
                textarea.replace_range(cursor..end, "");
                persistent.register = ch;
                persistent.register_is_linewise = false;
                persistent.last_change = Some(super::RecordedChange::DeleteChar { count: 1 });
            }
            VimAction::Handled
        }
        'r' => {
            *command = CommandState::Replace { count: 1 };
            VimAction::Handled
        }
        'p' => {
            operators::put_after(textarea, persistent);
            persistent.last_change = Some(super::RecordedChange::Put { before: false });
            VimAction::Handled
        }
        'P' => {
            operators::put_before(textarea, persistent);
            persistent.last_change = Some(super::RecordedChange::Put { before: true });
            VimAction::Handled
        }
        'u' => {
            // Undo: pop the most recent pre-edit snapshot. Wiring layer
            // already skips committing for 'u' so there's no redo-target
            // double-push to worry about.
            textarea.undo();
            VimAction::Handled
        }
        '.' => {
            // Dot-repeat: replay the last recorded change.
            if let Some(change) = persistent.last_change.clone() {
                replay_change(textarea, &change, persistent);
            }
            VimAction::Handled
        }

        // Find
        'f' => {
            *command = CommandState::Find {
                find: FindType::F,
                count: 1,
            };
            VimAction::Handled
        }
        'F' => {
            *command = CommandState::Find {
                find: FindType::BigF,
                count: 1,
            };
            VimAction::Handled
        }
        't' => {
            *command = CommandState::Find {
                find: FindType::T,
                count: 1,
            };
            VimAction::Handled
        }
        'T' => {
            *command = CommandState::Find {
                find: FindType::BigT,
                count: 1,
            };
            VimAction::Handled
        }

        // Repeat last find
        ';' => {
            if let Some((find, ch)) = persistent.last_find {
                let was = persistent.last_find;
                execute_find(ch, textarea, find, persistent);
                persistent.last_find = was;
            }
            VimAction::Handled
        }
        ',' => {
            if let Some((find, ch)) = persistent.last_find {
                let reverse = match find {
                    FindType::F => FindType::BigF,
                    FindType::BigF => FindType::F,
                    FindType::T => FindType::BigT,
                    FindType::BigT => FindType::T,
                };
                let was = persistent.last_find;
                execute_find(ch, textarea, reverse, persistent);
                persistent.last_find = was;
            }
            VimAction::Handled
        }

        // G prefix
        'g' => {
            *command = CommandState::G { count: 1 };
            VimAction::Handled
        }

        // Count
        '1'..='9' => {
            *command = CommandState::Count {
                digits: ch.to_string(),
            };
            VimAction::Handled
        }

        // Enter submits in normal mode
        '\n' | '\r' => VimAction::Submit,

        _ => VimAction::Unhandled,
    }
}

fn process_motion_with_count(
    ch: char,
    textarea: &mut TextArea,
    count: usize,
    command: &mut CommandState,
    persistent: &mut PersistentState,
) -> VimAction {
    for _ in 0..count {
        match process_idle_key(ch, textarea, command, persistent) {
            VimAction::Handled => {}
            other => return other,
        }
    }
    VimAction::Handled
}

fn process_operator_motion(
    ch: char,
    textarea: &mut TextArea,
    op: Operator,
    count: usize,
    command: &mut CommandState,
    persistent: &mut PersistentState,
) -> VimAction {
    match ch {
        // Motion-driven ranges
        'w' | 'e' | 'b' | 'h' | 'l' | '0' | '$' | '^' | 'G' => {
            let start = textarea.cursor();
            let target = apply_motion_count(textarea.text(), start, ch, count.max(1));
            let range = motion_range(textarea.text(), start, target, ch);
            operators::apply_operator(textarea, op, range, persistent);
            persistent.last_change = Some(super::RecordedChange::OperatorMotion {
                op,
                motion: ch,
                count,
            });
            command.reset();
            if op == Operator::Change {
                VimAction::EnterInsert
            } else {
                VimAction::Handled
            }
        }
        // Doubled operator = linewise (dd / cc / yy)
        'd' if op == Operator::Delete => {
            operators::delete_line(textarea, persistent);
            persistent.last_change = Some(super::RecordedChange::OperatorLine {
                op: Operator::Delete,
                count,
            });
            command.reset();
            VimAction::Handled
        }
        'c' if op == Operator::Change => {
            operators::change_line(textarea, persistent);
            persistent.last_change = Some(super::RecordedChange::OperatorLine {
                op: Operator::Change,
                count,
            });
            command.reset();
            VimAction::EnterInsert
        }
        'y' if op == Operator::Yank => {
            operators::yank_line(textarea, persistent);
            persistent.last_change = Some(super::RecordedChange::OperatorLine {
                op: Operator::Yank,
                count,
            });
            command.reset();
            VimAction::Handled
        }
        // Text-object scope (`i`/`a` followed by an object key)
        'i' => {
            *command = CommandState::OperatorTextObj {
                op,
                count,
                scope: TextObjScope::Inner,
            };
            VimAction::Handled
        }
        'a' => {
            *command = CommandState::OperatorTextObj {
                op,
                count,
                scope: TextObjScope::Around,
            };
            VimAction::Handled
        }
        // Find within operator
        'f' => {
            *command = CommandState::OperatorFind {
                op,
                count,
                find: FindType::F,
            };
            VimAction::Handled
        }
        'F' => {
            *command = CommandState::OperatorFind {
                op,
                count,
                find: FindType::BigF,
            };
            VimAction::Handled
        }
        't' => {
            *command = CommandState::OperatorFind {
                op,
                count,
                find: FindType::T,
            };
            VimAction::Handled
        }
        'T' => {
            *command = CommandState::OperatorFind {
                op,
                count,
                find: FindType::BigT,
            };
            VimAction::Handled
        }
        // Count within operator (e.g. `d3w`)
        '0'..='9' => {
            *command = CommandState::OperatorCount {
                op,
                count,
                digits: ch.to_string(),
            };
            VimAction::Handled
        }
        // `g` within operator (e.g. `dgg`)
        'g' => {
            *command = CommandState::OperatorG { op, count };
            VimAction::Handled
        }
        _ => {
            command.reset();
            VimAction::Unhandled
        }
    }
}

fn apply_motion_count(text: &str, start: usize, ch: char, count: usize) -> usize {
    let mut target = start;
    for _ in 0..count {
        target = match ch {
            'w' => motions::word_forward(text, target),
            'e' => motions::word_end(text, target),
            'b' => motions::word_backward(text, target),
            'h' => prev_char_boundary(text, target),
            'l' => next_char_boundary(text, target),
            '0' => motions::line_start(text, target),
            '$' => motions::line_end(text, target),
            '^' => motions::first_non_blank(text, target),
            'G' => motions::go_to_bottom(text),
            _ => target,
        };
    }
    target
}

/// Build the deletion range for a motion.
///
/// - Forward inclusive motions (`e`, `$`, `f`, `t`, `G`) include the
///   target character: range = `[cursor, next_char_after(target))`.
/// - Forward exclusive motions (`w`, `l`, `0`, `^`) stop at the target:
///   range = `[cursor, target)`.
/// - Backward motions (`b`, `h`, `F`, `T`) — target is the new lo bound,
///   cursor is exclusive at hi: range = `[target, cursor)`. Vim's `dF`/`dT`
///   match this convention.
fn motion_range(text: &str, start: usize, target: usize, motion: char) -> Range<usize> {
    if target >= start {
        let inclusive_forward = matches!(motion, 'e' | '$' | 'f' | 't' | 'G');
        let hi = if inclusive_forward && target < text.len() {
            next_char_boundary(text, target)
        } else {
            target
        };
        start..hi
    } else {
        target..start
    }
}

fn execute_find(
    ch: char,
    textarea: &mut TextArea,
    find: FindType,
    persistent: &mut PersistentState,
) {
    if let Some(target) = find_target(textarea, find, ch) {
        textarea.set_cursor(target);
    }
    persistent.last_find = Some((find, ch));
}

fn find_operator_range(textarea: &TextArea, find: FindType, ch: char) -> Option<Range<usize>> {
    let start = textarea.cursor();
    let target = find_target(textarea, find, ch)?;
    // Re-use `motion_range` by mapping the FindType back to its motion char.
    let motion = match find {
        FindType::F => 'f',
        FindType::BigF => 'F',
        FindType::T => 't',
        FindType::BigT => 'T',
    };
    Some(motion_range(textarea.text(), start, target, motion))
}

fn find_target(textarea: &TextArea, find: FindType, ch: char) -> Option<usize> {
    let text = textarea.text();
    let pos = textarea.cursor();
    match find {
        FindType::F => motions::find_char_forward(text, pos, ch),
        FindType::BigF => motions::find_char_backward(text, pos, ch),
        FindType::T => motions::till_char_forward(text, pos, ch),
        FindType::BigT => motions::till_char_backward(text, pos, ch),
    }
}

fn resolve_text_object(
    ch: char,
    text: &str,
    pos: usize,
    scope: TextObjScope,
) -> Option<Range<usize>> {
    match ch {
        'w' => text_objects::word(text, pos, scope),
        '"' => text_objects::quoted(text, pos, '"', scope),
        '\'' => text_objects::quoted(text, pos, '\'', scope),
        '(' | ')' | 'b' => text_objects::bracket(text, pos, '(', ')', scope),
        '{' | '}' | 'B' => text_objects::bracket(text, pos, '{', '}', scope),
        '[' | ']' => text_objects::bracket(text, pos, '[', ']', scope),
        _ => None,
    }
}

/// Replay a recorded change against the current cursor (vim `.`).
fn replay_change(
    textarea: &mut TextArea,
    change: &super::RecordedChange,
    persistent: &mut PersistentState,
) {
    use super::RecordedChange;
    match change {
        RecordedChange::OperatorMotion { op, motion, count } => {
            let start = textarea.cursor();
            let target = apply_motion_count(textarea.text(), start, *motion, (*count).max(1));
            let range = motion_range(textarea.text(), start, target, *motion);
            operators::apply_operator(textarea, *op, range, persistent);
        }
        RecordedChange::OperatorLine { op, count: _ } => match op {
            Operator::Delete => operators::delete_line(textarea, persistent),
            Operator::Change => operators::change_line(textarea, persistent),
            Operator::Yank => operators::yank_line(textarea, persistent),
        },
        RecordedChange::DeleteChar { count } => {
            for _ in 0..(*count).max(1) {
                let cursor = textarea.cursor();
                let end = next_char_boundary(textarea.text(), cursor);
                if end > cursor {
                    let ch = textarea.text()[cursor..end].to_string();
                    textarea.replace_range(cursor..end, "");
                    persistent.register = ch;
                    persistent.register_is_linewise = false;
                }
            }
        }
        RecordedChange::ReplaceChar { ch, count: _ } => {
            operators::replace_char(textarea, *ch);
        }
        RecordedChange::Put { before } => {
            if *before {
                operators::put_before(textarea, persistent);
            } else {
                operators::put_after(textarea, persistent);
            }
        }
    }
}
