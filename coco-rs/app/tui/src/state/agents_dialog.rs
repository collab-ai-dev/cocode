//! `/agents` dialog state.
//!
//! Two-tab overlay (Running + Library). Lives in its own module rather
//! than extending `surface_payloads.rs`
//! (already 1289 LoC) per the workspace's "no files > 1600 LoC" rule.
//! Data sources:
//!   - **Library**: snapshot of `coco_subagent::AgentDefinitionStore`
//!     projected to `LibraryRow`, grouped by `AgentSource`. Refreshed on
//!     dialog open and after every CRUD operation.
//!   - **Running**: lives on `SessionState.subagents` (already mirrored
//!     by the background-pills bar). Read directly at render time —
//!     dialog state only holds the cursor position.
//!
//! The dialog title is `"Agents"` (a static label rendered above the
//! tab strip — NOT a third tab). Tabs are `"Running"` and `"Library"`.

use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

use coco_types::AgentColorName;
use coco_types::AgentSource;

/// Which tab is currently focused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentsDialogTab {
    Running,
    Library,
}

impl AgentsDialogTab {
    /// Opens with the Running tab selected by default.
    pub const DEFAULT: Self = Self::Running;

    /// Cycle to the next tab — wraps. `delta > 0` is right (`→`),
    /// `delta < 0` is left (`←`).
    pub fn cycled(self, delta: i32) -> Self {
        let order = [Self::Running, Self::Library];
        let idx = order.iter().position(|t| *t == self).unwrap_or(0) as i32;
        let next = (idx + delta).rem_euclid(order.len() as i32) as usize;
        order[next]
    }
}

/// One row in the Library tab. Source headers and the "Create new"
/// sentinel are interleaved with agent rows so the cursor walks a
/// single flat sequence and the renderer doesn't have to track
/// per-section state.
#[derive(Debug, Clone)]
pub enum LibraryRow {
    /// First row in the list. Selecting it opens the inline 4-step
    /// create wizard.
    CreateNew,
    /// Group header (rendered dim, not selectable). One per source
    /// that has at least one agent. The renderer skips these when
    /// walking selection.
    SourceHeader { label: String },
    /// An individual agent row. Selectable.
    Agent {
        /// Canonical `agent_type` identifier — drives lookups against
        /// the live store on Enter / Edit / Delete.
        name: String,
        /// `whenToUse` description, truncated on render.
        description: Option<String>,
        /// Source group this row belongs to. Drives precedence display,
        /// inline source label (`name · source` row layout), and
        /// "cannot be modified" hint for built-in rows.
        source: AgentSource,
        /// Optional badge color from frontmatter. Renderer uses it for
        /// the row tint; `None` falls back to the theme default.
        color: Option<AgentColorName>,
        /// `true` for built-in entries — the renderer dims them and
        /// adds the "cannot be modified" hint.
        is_builtin: bool,
        /// `true` when this `agent_type` is shadowed by a higher-
        /// priority source. Renderer adds an "(overridden)" suffix.
        is_overridden: bool,
        /// Number of currently-running invocations of this agent —
        /// drives the `· N running` badge.
        running_count: u32,
        /// Absolute markdown source path. `None` for built-in /
        /// plugin / in-memory entries that aren't editable. Drives
        /// the Library tab's Enter (edit) and `d` (delete) actions.
        source_path: Option<PathBuf>,
    },
}

impl LibraryRow {
    /// `true` for rows the cursor may stop on. Headers and sentinels
    /// stay focusable when they have a real action (CreateNew); pure
    /// labels do not.
    pub fn is_selectable(&self) -> bool {
        !matches!(self, Self::SourceHeader { .. })
    }

    /// Short inline source label used by the Library row renderer.
    /// Lowercase to keep alignment compact.
    pub fn short_source_label(source: AgentSource) -> &'static str {
        match source {
            AgentSource::UserSettings => "user",
            AgentSource::ProjectSettings => "project",
            AgentSource::PolicySettings => "managed",
            AgentSource::Plugin => "plugin",
            AgentSource::FlagSettings => "flag",
            AgentSource::BuiltIn => "built-in",
        }
    }
}

/// Toast key dispatched by the Library tab's submit classifier.
/// Kept as a typed enum (not `&'static str`) so a new toast surface
/// fails to compile until the render side adds an arm — same pattern
/// as [`WizardError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryToastKind {
    /// Pressing Enter on a built-in agent row. Surfaces the
    /// override-by-creating-a-file hint.
    BuiltinReadOnly,
    /// Pressing Enter on a row whose `source_path` is `None`
    /// (plugin / in-memory / unknown). Distinct from
    /// [`Self::BuiltinReadOnly`] only in framing.
    NoFile,
}

/// Inline 4-step create wizard. `name` → `description` → `source` →
/// `confirm`, then a `UserCommand::CreateAgent` dispatch writes the
/// markdown file and the CLI bridge opens it in `$EDITOR` for further
/// tuning.
///
/// Per the A3 折中 choice: minimal inline form for create (so a
/// new-agent flow stays in-dialog), `$EDITOR` for edit (less code,
/// power-user-friendly). The wizard intentionally does NOT collect
/// tools / model / memory — those fields default in the template and
/// the user tunes them in the editor session. Color is auto-assigned
/// from the next-unused palette entry (see
/// `coco_subagent::next_unused_color`).
#[derive(Debug, Clone)]
pub struct CreateWizardState {
    pub step: CreateWizardStep,
    /// Name field with full cursor support (insert / delete at
    /// arbitrary position, Home / End / Left / Right). Validated by
    /// [`validate_agent_name`] on Enter at the Name step.
    pub name: WizardTextField,
    /// `whenToUse` description with full cursor support. Required
    /// and non-empty on Enter.
    pub description: WizardTextField,
    /// Currently-highlighted source on the Source step. Limited to
    /// User and Project — the writable scopes coco-rs supports
    /// without elevation. Built-in / Policy / Flag / Plugin are
    /// excluded because the dialog doesn't own those filesystem
    /// roots.
    pub source: WizardSource,
    /// Typed validation / disposition diagnostic to display under
    /// the current input. Cleared when the user changes the value.
    pub error: Option<WizardError>,
}

/// Editable text field with byte-aware cursor positioning. Used by
/// the wizard's Name and Description steps so the user can insert /
/// delete anywhere in the input — replacement for the previous
/// append-only `String` field.
///
/// Cursor is tracked as a **char count** rather than a byte index so
/// the public API stays UTF-8-safe (each method converts to a byte
/// index at the boundary).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WizardTextField {
    /// Current text content.
    pub text: String,
    /// Number of characters before the cursor. `0 ⇒ cursor at start`,
    /// `text.chars().count() ⇒ cursor at end`.
    pub cursor: usize,
}

impl WizardTextField {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a field with the cursor parked at the end of `text`.
    /// Useful for tests that want to verify cursor-aware editing
    /// against a pre-populated value. Named `seeded` (rather than
    /// `from_str`) so it doesn't collide with `std::str::FromStr`.
    pub fn seeded(text: &str) -> Self {
        let cursor = text.chars().count();
        Self {
            text: text.to_string(),
            cursor,
        }
    }

    /// Character count of [`Self::text`]. Used by the renderer for
    /// caret placement and by `move_end`.
    pub fn char_len(&self) -> usize {
        self.text.chars().count()
    }

    /// Insert `c` at the cursor position and advance the cursor by
    /// one character.
    pub fn insert_char(&mut self, c: char) {
        let byte = self.byte_idx_at(self.cursor);
        self.text.insert(byte, c);
        self.cursor += 1;
    }

    /// Remove the character before the cursor. No-op when the
    /// cursor is at position 0.
    pub fn delete_back(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let target = self.cursor - 1;
        let byte = self.byte_idx_at(target);
        self.text.remove(byte);
        self.cursor = target;
    }

    /// Remove the character at the cursor (Delete key). No-op at
    /// end-of-text.
    pub fn delete_forward(&mut self) {
        let byte = self.byte_idx_at(self.cursor);
        if byte < self.text.len() {
            self.text.remove(byte);
        }
    }

    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    pub fn move_right(&mut self) {
        let total = self.char_len();
        if self.cursor < total {
            self.cursor += 1;
        }
    }

    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.char_len();
    }

    /// Split the text at the cursor for rendering. Returns
    /// `(before, after)` slices into [`Self::text`].
    pub fn split_at_cursor(&self) -> (&str, &str) {
        let byte = self.byte_idx_at(self.cursor);
        self.text.split_at(byte)
    }

    fn byte_idx_at(&self, char_idx: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_idx)
            .map(|(b, _)| b)
            .unwrap_or(self.text.len())
    }
}

/// Typed wizard error. The renderer maps the variant to a localized
/// string — compile-time exhaustiveness ensures new variants land in
/// the catalog alongside the code that surfaces them.
///
/// Note: the CLI bridge's `prepare_agent_create` may also fail
/// (permission denied, disk full, …) but those land on a toast via
/// `TuiOnlyEvent::PromptEditorFailed` — they never bind into a
/// wizard error because by then the wizard is closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WizardError {
    /// Name input is empty after trim.
    NameEmpty,
    /// Name doesn't start with an ASCII letter.
    NameLead,
    /// Name contains characters outside `[A-Za-z0-9_-]`.
    NameChars,
    /// Description is empty after trim.
    DescEmpty,
    /// Target markdown file already exists on disk.
    AlreadyExists { path: PathBuf },
    /// Defensive: the wizard's source selection resolved to a non-
    /// writable [`AgentSource`]. Unreachable under the current
    /// wizard (it restricts to User / Project) but surfaces an
    /// observable diagnostic if a future widening regresses the
    /// invariant.
    NonWritableSource,
}

impl CreateWizardState {
    pub fn new() -> Self {
        Self {
            step: CreateWizardStep::Name,
            name: WizardTextField::new(),
            description: WizardTextField::new(),
            source: WizardSource::User,
            error: None,
        }
    }

    /// Borrow the text field for the active step. Returns `None`
    /// when the active step doesn't hold a text input (Source /
    /// Confirm).
    pub fn active_field(&self) -> Option<&WizardTextField> {
        match self.step {
            CreateWizardStep::Name => Some(&self.name),
            CreateWizardStep::Description => Some(&self.description),
            CreateWizardStep::Source | CreateWizardStep::Confirm => None,
        }
    }

    pub fn active_field_mut(&mut self) -> Option<&mut WizardTextField> {
        match self.step {
            CreateWizardStep::Name => Some(&mut self.name),
            CreateWizardStep::Description => Some(&mut self.description),
            CreateWizardStep::Source | CreateWizardStep::Confirm => None,
        }
    }
}

impl Default for CreateWizardState {
    fn default() -> Self {
        Self::new()
    }
}

/// Wizard step. Linear forward / back via Enter / Esc.
///
/// Four steps: `Name` → `Description` → `Source` → `Confirm`, so the
/// user can review inputs before the irreversible filesystem write.
/// Tools / model / memory are left to `$EDITOR` per the A3 折中
/// decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateWizardStep {
    Name,
    Description,
    Source,
    Confirm,
}

/// Source choices offered by the create wizard. Maps to
/// `coco_types::AgentSource` at dispatch time but kept narrower to
/// signal that the writable surface is intentionally limited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardSource {
    User,
    Project,
}

impl WizardSource {
    pub fn as_agent_source(self) -> AgentSource {
        match self {
            Self::User => AgentSource::UserSettings,
            Self::Project => AgentSource::ProjectSettings,
        }
    }

    pub fn cycled(self, delta: i32) -> Self {
        let order = [Self::User, Self::Project];
        let idx = order.iter().position(|s| *s == self).unwrap_or(0) as i32;
        let next = (idx + delta).rem_euclid(order.len() as i32) as usize;
        order[next]
    }
}

/// Returns `true` when `c` is acceptable as a Name-input character.
/// The wizard rejects invalid Name chars at input time so the user
/// gets immediate visual feedback rather than a deferred Enter-time
/// error message. Whitespace, punctuation, and non-ASCII letters are
/// all rejected here.
pub fn is_valid_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

/// Returns `true` when `c` is acceptable inside the Description
/// field. Newlines, tabs, and other control characters are dropped
/// so the YAML body the wizard emits stays on a single physical
/// line. Everything else (including non-ASCII Unicode) is allowed.
pub fn is_valid_desc_char(c: char) -> bool {
    !c.is_control()
}

/// Validate the final Name string at Enter time. Catches the
/// "first character must be alphabetic" rule that the per-char
/// filter cannot enforce on its own. Empty / whitespace-only input
/// is also caught here so the user sees a clear error rather than a
/// silently-blank Enter.
pub fn validate_agent_name(name: &str) -> Result<(), WizardError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(WizardError::NameEmpty);
    }
    let mut chars = trimmed.chars();
    // `chars` is non-empty here — the `trimmed.is_empty()` guard above
    // already rejected the zero-character case. Match-and-unreachable
    // keeps the lint happy without paying for an extra branch.
    let Some(first) = chars.next() else {
        unreachable!("trimmed already checked non-empty");
    };
    if !first.is_ascii_alphabetic() {
        return Err(WizardError::NameLead);
    }
    if !chars.all(is_valid_name_char) {
        return Err(WizardError::NameChars);
    }
    Ok(())
}

/// Pure-logic resolver for the wizard's target markdown path. Both
/// the TUI pre-flight (`update/agents_dialog.rs::wizard_finalize`)
/// and the CLI bridge (`tui_runner.rs::prepare_agent_create`) call
/// this with their respective `cwd` / `config_home` snapshots so the
/// two stay in lock-step.
///
/// Returns the absolute target path, or a typed `WizardError` for
/// the surfaces the wizard knows how to render:
///   - [`WizardError::NonWritableSource`] when the source isn't one
///     of User / Project,
///   - [`WizardError::AlreadyExists`] when a markdown file at the
///     target path is already present on disk.
///
/// Filesystem I/O is restricted to a single `path.exists()` call —
/// the caller is responsible for `create_dir_all` + `write` since
/// those want the async / blocking-pool wrapper.
pub fn resolve_create_target(
    source: AgentSource,
    name: &str,
    cwd: &Path,
    config_home: &Path,
) -> Result<PathBuf, WizardError> {
    let dir = writable_agent_dir(source, cwd, config_home)?;
    let path = dir.join(format!("{name}.md"));
    if path.exists() {
        return Err(WizardError::AlreadyExists { path });
    }
    Ok(path)
}

/// Resolve the on-disk directory for an [`AgentSource`] that the
/// wizard is allowed to write to. Wraps
/// [`coco_subagent::resolve_writable_agent_dir`] with a typed
/// [`WizardError`] for the non-writable case so the wizard can
/// surface it inline.
pub fn writable_agent_dir(
    source: AgentSource,
    cwd: &Path,
    config_home: &Path,
) -> Result<PathBuf, WizardError> {
    coco_subagent::resolve_writable_agent_dir(source, config_home, cwd)
        .ok_or(WizardError::NonWritableSource)
}

/// `/agents` dialog payload. Mutated by `update/agents_dialog.rs`;
/// rendered by `presentation/agents_dialog::agents_dialog_content`.
#[derive(Debug, Clone)]
pub struct AgentsDialogState {
    /// Currently focused tab.
    pub selected_tab: AgentsDialogTab,
    /// Cursor index into [`Self::library`]. Always points at a
    /// selectable row (header positions are skipped by nav).
    pub library_cursor: usize,
    /// Cursor index into the live `SessionState.subagents` list.
    /// Clamped at render time when the underlying list shrinks.
    pub running_cursor: usize,
    /// Flat list of rows for the Library tab. Ordering: CreateNew
    /// first, then each source group (header + rows) in order
    /// (User, Project, Local, Managed, Plugin, CLI, Built-in).
    /// Built-in rows always render last with `cannot be modified`.
    pub library: Vec<LibraryRow>,
    /// `agent_type` of every agent the user has invoked in this
    /// session. Promoted above the source-grouped order.
    pub used_this_session: BTreeSet<String>,
    /// Inline create wizard. `Some` ⇒ the Library tab renders the
    /// wizard instead of the list. `None` ⇒ list view.
    pub wizard: Option<CreateWizardState>,
    /// Pending delete confirmation. `Some(path)` ⇒ the Library tab
    /// renders a `Delete <name>? (y/n)` prompt and swallows nav until
    /// the user confirms (`y`/Enter → dispatch) or cancels (`n`/Esc).
    /// Guards the destructive `DeleteAgentFile` command behind explicit
    /// confirmation (TS parity: `AgentsMenu` `delete-confirm` mode).
    pub pending_delete: Option<PathBuf>,
}

impl AgentsDialogState {
    pub fn new(library: Vec<LibraryRow>) -> Self {
        let mut state = Self {
            selected_tab: AgentsDialogTab::DEFAULT,
            library_cursor: 0,
            running_cursor: 0,
            library,
            used_this_session: BTreeSet::new(),
            wizard: None,
            pending_delete: None,
        };
        // Snap the cursor onto the first selectable row (skip past
        // any leading header). Defensive: even with `CreateNew` first
        // this is a no-op since CreateNew is selectable.
        state.library_cursor = state.first_selectable_index().unwrap_or(0);
        state
    }

    /// Enter the inline create wizard. Switches the Library tab from
    /// the list view to the wizard form. Tab switching and `Esc`
    /// inside the wizard are routed back through `wizard` rather
    /// than dismissing the modal.
    pub fn open_wizard(&mut self) {
        self.wizard = Some(CreateWizardState::new());
    }

    /// Drop the active wizard (Esc on step 1, or post-confirm cleanup).
    pub fn close_wizard(&mut self) {
        self.wizard = None;
    }

    /// `true` when the Library tab is currently in wizard mode.
    pub fn is_in_wizard(&self) -> bool {
        self.wizard.is_some()
    }

    /// Advance the cursor through selectable rows only. Headers are
    /// skipped transparently.
    pub fn nav_library(&mut self, delta: i32) {
        if self.library.is_empty() {
            return;
        }
        let step: i32 = delta.signum();
        if step == 0 {
            return;
        }
        let mut idx = self.library_cursor as i32;
        let len = self.library.len() as i32;
        for _ in 0..delta.unsigned_abs() {
            let mut next = (idx + step).rem_euclid(len);
            // Skip non-selectable rows (headers). Walk up to `len`
            // times so a slot consisting only of headers terminates.
            for _ in 0..len {
                if self.library[next as usize].is_selectable() {
                    break;
                }
                next = (next + step).rem_euclid(len);
            }
            idx = next;
        }
        self.library_cursor = idx as usize;
    }

    /// Clamp + skip-headers helper used after the library list is
    /// rebuilt (CRUD round-trip).
    pub fn snap_library_cursor(&mut self) {
        if self.library.is_empty() {
            self.library_cursor = 0;
            return;
        }
        if self.library_cursor >= self.library.len() {
            self.library_cursor = self.library.len() - 1;
        }
        if !self.library[self.library_cursor].is_selectable() {
            self.library_cursor = self.first_selectable_index().unwrap_or(0);
        }
    }

    fn first_selectable_index(&self) -> Option<usize> {
        self.library.iter().position(LibraryRow::is_selectable)
    }

    /// Currently-focused library row, or `None` when the list is empty.
    pub fn focused_library(&self) -> Option<&LibraryRow> {
        self.library.get(self.library_cursor)
    }
}

#[cfg(test)]
#[path = "agents_dialog.test.rs"]
mod tests;
