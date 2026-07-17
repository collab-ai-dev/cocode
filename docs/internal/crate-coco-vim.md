# coco-vim — Crate Plan

Directory: `app/tui/` (vim submodule, v2)
TS source: `vim/` (5 files, ~1.5K LOC), `commands/vim/` (2 files, 51 LOC)

## Dependencies

```
coco-vim depends on:
  - unicode-segmentation (grapheme-aware string operations)

coco-vim does NOT depend on:
  - coco-tui      (TUI consumes vim, not the reverse)
  - coco-types    (no message/tool types needed)
  - coco-config   (vim enabled flag read by coco-tui, not vim itself)
  - any internal coco-* crate (pure state machine library)
```

## Data Definitions

```rust
/// Top-level vim state: INSERT or NORMAL mode.
pub enum VimState {
    Insert { inserted_text: String },
    Normal { command: CommandState },
}

/// State machine with 10 states for command parsing.
pub enum CommandState {
    Idle,
    Count { digits: String },
    Operator { op: Operator, count: i32 },
    OperatorCount { op: Operator, pre_count: i32, digits: String },
    OperatorFind { op: Operator, count: i32, find_type: FindType },
    OperatorTextObj { op: Operator, count: i32, scope: TextObjScope },
    Find { find_type: FindType, count: i32 },
    G { count: i32 },
    OperatorG { op: Operator, count: i32 },
    Replace { count: i32 },
    Indent { direction: IndentDirection, count: i32 },
}

#[derive(Clone, Copy)]
pub enum Operator { Delete, Change, Yank }

#[derive(Clone, Copy)]
pub enum FindType { F, FBack, T, TBack }

#[derive(Clone, Copy)]
pub enum TextObjScope { Inner, Around }

#[derive(Clone, Copy)]
pub enum IndentDirection { Right, Left }

/// Persistent state across commands (dot-repeat, registers, last find).
pub struct PersistentState {
    pub last_change: Option<RecordedChange>,
    pub last_find: Option<(FindType, char)>,
    pub register: String,
    pub register_is_linewise: bool,
}

/// Recorded change for dot-repeat (.).
pub enum RecordedChange {
    OperatorMotion { op: Operator, motion: String, count: i32 },
    OperatorFind { op: Operator, find_type: FindType, char: char, count: i32 },
    OperatorTextObj { op: Operator, scope: TextObjScope, obj_type: String, count: i32 },
    LineOp { op: Operator, count: i32 },
    DeleteChar { count: i32 },
    ReplaceChar { char: char, count: i32 },
    ToggleCase { count: i32 },
    JoinLines { count: i32 },
    Insert { text: String },
}

/// Cursor position in text buffer.
pub struct Cursor {
    pub offset: usize,
    pub line: usize,
    pub col: usize,
}
```

## Core Logic

### Motions (from `motions.ts`, 82 LOC)

```rust
/// Calculate cursor position after applying a motion with count.
pub fn resolve_motion(key: &str, cursor: &Cursor, count: i32, text: &str) -> Cursor;

/// Whether the motion includes the target character (e, E, $).
pub fn is_inclusive_motion(key: &str) -> bool;

/// Whether the motion operates on full lines (j, k, G, gg).
pub fn is_linewise_motion(key: &str) -> bool;
```

Supported: h, l, j, k, w, b, e, W, B, E, 0, ^, $, G, gg, gj, gk

### Operators (from `operators.ts`, 556 LOC)

```rust
/// Execution context injected by TUI — decouples vim from editor implementation.
pub trait OperatorContext {
    fn get_text(&self) -> &str;
    fn set_text(&mut self, text: &str);
    fn get_offset(&self) -> usize;
    fn set_offset(&mut self, offset: usize);
    fn enter_insert(&mut self);
    fn get_register(&self) -> (&str, bool);
    fn set_register(&mut self, text: &str, linewise: bool);
    fn get_last_find(&self) -> Option<(FindType, char)>;
    fn set_last_find(&mut self, find_type: FindType, char: char);
    fn record_change(&mut self, change: RecordedChange);
}

pub fn execute_operator_motion(op: Operator, motion: &str, count: i32, ctx: &mut dyn OperatorContext);
pub fn execute_operator_find(op: Operator, ft: FindType, ch: char, count: i32, ctx: &mut dyn OperatorContext);
pub fn execute_operator_text_obj(op: Operator, scope: TextObjScope, obj: &str, count: i32, ctx: &mut dyn OperatorContext);
pub fn execute_line_op(op: Operator, count: i32, ctx: &mut dyn OperatorContext);
pub fn execute_x(count: i32, ctx: &mut dyn OperatorContext);
pub fn execute_replace(ch: char, count: i32, ctx: &mut dyn OperatorContext);
pub fn execute_toggle_case(count: i32, ctx: &mut dyn OperatorContext);
pub fn execute_join(count: i32, ctx: &mut dyn OperatorContext);
pub fn execute_paste(after: bool, count: i32, ctx: &mut dyn OperatorContext);
pub fn execute_indent(dir: IndentDirection, count: i32, ctx: &mut dyn OperatorContext);
pub fn execute_open_line(below: bool, ctx: &mut dyn OperatorContext);
```

### Text Objects (from `textObjects.ts`, 186 LOC)

```rust
pub struct TextObjectRange {
    pub start: usize,
    pub end: usize,
}

/// Find text object boundaries around cursor.
/// Types: w, W (words), (), [], {}, <> (brackets), ", ', ` (quotes)
pub fn find_text_object(
    text: &str,
    offset: usize,
    obj_type: &str,
    scope: TextObjScope,
) -> Option<TextObjectRange>;
```

### Transitions (from `transitions.ts`, 490 LOC)

```rust
pub struct TransitionResult {
    pub next: Option<CommandState>,
    pub execute: Option<Box<dyn FnOnce(&mut dyn OperatorContext)>>,
}

/// Main dispatcher: routes input based on current CommandState.
/// Exhaustive match over all 10 states.
pub fn transition(
    state: &CommandState,
    input: &str,
    persistent: &PersistentState,
) -> TransitionResult;
```

Key behaviors:
- Count multiplication: `2d5w` = delete 10 words
- Double-key line ops: `dd`, `cc`, `yy`
- Repeat: `.` (dot-repeat via RecordedChange), `;`/`,` (repeat-find)
- Grapheme-aware string operations (unicode-segmentation crate)

## Module Layout

```
vim/
  mod.rs              — pub mod, VimState/PersistentState constructors
  types.rs            — all enums, structs, Cursor
  motions.rs          — resolve_motion, motion classification
  operators.rs        — OperatorContext trait, all execute_* functions
  text_objects.rs     — find_text_object (words, brackets, quotes)
  transitions.rs      — state machine dispatcher
```
