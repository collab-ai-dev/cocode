//! `/permissions` rule-editor overlay state.
//!
//! Tabbed overlay (Allow / Ask / Deny / Workspace). The shape mirrors
//! the `/agents` dialog (tab strip + per-tab cursor + inline forms) and
//! reuses [`WizardTextField`] for the add-rule / add-directory text inputs
//! and [`PermissionAskChoice`]-style option rows for the destination
//! selector.
//!
//! Data is a CLI-built snapshot pushed via
//! `TuiOnlyEvent::OpenPermissionsEditor`; every mutation round-trips
//! through `UserCommand::ApplyPermissionUpdate` and a fresh payload
//! re-render, so the overlay never edits the on-disk truth in place.

use coco_types::PermissionBehavior;
use coco_types::PermissionRuleSource;
use coco_types::PermissionRuleValue;
use coco_types::PermissionUpdateDestination;
use coco_types::PermissionsEditorPayload;

use crate::state::WizardTextField;

/// Which tab is focused. Allow / Ask / Deny list rules of the matching
/// behavior; Workspace lists additional working directories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionsEditorTab {
    Allow,
    Ask,
    Deny,
    Workspace,
}

impl PermissionsEditorTab {
    /// The rule list opens on the Allow tab by default.
    pub const DEFAULT: Self = Self::Allow;
    pub const ORDER: [Self; 4] = [Self::Allow, Self::Ask, Self::Deny, Self::Workspace];

    /// Cycle to the next tab — wraps. `delta > 0` is right (`→`).
    pub fn cycled(self, delta: i32) -> Self {
        let idx = Self::ORDER.iter().position(|t| *t == self).unwrap_or(0) as i32;
        let next = (idx + delta).rem_euclid(Self::ORDER.len() as i32) as usize;
        Self::ORDER[next]
    }

    /// Behavior whose rules this tab lists, or `None` for Workspace.
    pub fn behavior(self) -> Option<PermissionBehavior> {
        match self {
            Self::Allow => Some(PermissionBehavior::Allow),
            Self::Ask => Some(PermissionBehavior::Ask),
            Self::Deny => Some(PermissionBehavior::Deny),
            Self::Workspace => None,
        }
    }
}

/// Writable persist destination for a rule source, or `None` for the
/// read-only / in-memory sources (policy, CLI, flag, session, command).
/// The editor uses this both to gate deletion and to map a row's source
/// back to the settings file it must rewrite.
pub fn source_destination(source: PermissionRuleSource) -> Option<PermissionUpdateDestination> {
    match source {
        PermissionRuleSource::UserSettings => Some(PermissionUpdateDestination::UserSettings),
        PermissionRuleSource::ProjectSettings => Some(PermissionUpdateDestination::ProjectSettings),
        PermissionRuleSource::LocalSettings => Some(PermissionUpdateDestination::LocalSettings),
        PermissionRuleSource::FlagSettings
        | PermissionRuleSource::PolicySettings
        | PermissionRuleSource::CliArg
        | PermissionRuleSource::Command
        | PermissionRuleSource::Session => None,
    }
}

/// Short inline source tag used by row rendering. Lowercase, English-only.
pub fn short_source_label(source: PermissionRuleSource) -> &'static str {
    match source {
        PermissionRuleSource::UserSettings => "user",
        PermissionRuleSource::ProjectSettings => "project",
        PermissionRuleSource::LocalSettings => "local",
        PermissionRuleSource::FlagSettings => "flag",
        PermissionRuleSource::PolicySettings => "managed",
        PermissionRuleSource::CliArg => "cli",
        PermissionRuleSource::Command => "command",
        PermissionRuleSource::Session => "session",
    }
}

/// One rule row in an Allow / Ask / Deny tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermRuleRow {
    pub behavior: PermissionBehavior,
    pub source: PermissionRuleSource,
    pub tool_pattern: String,
    pub rule_content: Option<String>,
}

impl PermRuleRow {
    /// Canonical display string — `Bash(git *)` or bare `Read`.
    pub fn display(&self) -> String {
        match &self.rule_content {
            Some(content) => format!("{}({content})", self.tool_pattern),
            None => self.tool_pattern.clone(),
        }
    }

    /// `true` when the rule lives in a writable settings layer the editor
    /// can delete from (User / Project / Local).
    pub fn is_editable(&self) -> bool {
        source_destination(self.source).is_some()
    }

    pub fn to_value(&self) -> PermissionRuleValue {
        PermissionRuleValue {
            tool_pattern: self.tool_pattern.clone(),
            rule_content: self.rule_content.clone(),
        }
    }
}

/// One additional-working-directory row in the Workspace tab. The current
/// working directory is included as a read-only `is_cwd` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermDirRow {
    pub path: String,
    pub source: PermissionRuleSource,
    /// `true` for the read-only current-working-directory row (TS
    /// "Original working directory").
    pub is_cwd: bool,
}

impl PermDirRow {
    /// `true` when the directory can be removed (not the cwd row and
    /// contributed by a writable layer).
    pub fn is_editable(&self) -> bool {
        !self.is_cwd && source_destination(self.source).is_some()
    }
}

/// The three writable scopes offered by the destination selector
/// (project-local first, then project, then user).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorDestination {
    Local,
    Project,
    User,
}

impl EditorDestination {
    pub const ORDER: [Self; 3] = [Self::Local, Self::Project, Self::User];

    pub fn as_update_destination(self) -> PermissionUpdateDestination {
        match self {
            Self::Local => PermissionUpdateDestination::LocalSettings,
            Self::Project => PermissionUpdateDestination::ProjectSettings,
            Self::User => PermissionUpdateDestination::UserSettings,
        }
    }

    pub fn as_rule_source(self) -> PermissionRuleSource {
        match self {
            Self::Local => PermissionRuleSource::LocalSettings,
            Self::Project => PermissionRuleSource::ProjectSettings,
            Self::User => PermissionRuleSource::UserSettings,
        }
    }
}

/// Step of the inline add form. Shared by the add-rule (Allow/Ask/Deny)
/// and add-directory (Workspace) flows; the active tab decides whether
/// the input is a rule pattern or a directory path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddStep {
    /// Typing the rule pattern / directory path.
    Input,
    /// Picking the destination scope (Local / Project / User).
    Destination,
}

/// Typed validation diagnostic shown under the add-form input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermEditorError {
    /// Input is empty after trim.
    EmptyInput,
}

/// Inline add form for a new rule or directory. `Some` on the editor
/// state replaces the list with the form.
#[derive(Debug, Clone)]
pub struct AddForm {
    pub step: AddStep,
    pub input: WizardTextField,
    /// Index into [`EditorDestination::ORDER`].
    pub destination: usize,
    pub error: Option<PermEditorError>,
}

impl AddForm {
    pub fn new() -> Self {
        Self {
            step: AddStep::Input,
            input: WizardTextField::new(),
            destination: 0,
            error: None,
        }
    }

    pub fn selected_destination(&self) -> EditorDestination {
        EditorDestination::ORDER[self.destination.min(EditorDestination::ORDER.len() - 1)]
    }

    /// Move the destination highlight — wraps over the three scopes.
    pub fn nav_destination(&mut self, delta: i32) -> bool {
        let len = EditorDestination::ORDER.len() as i32;
        let next = (self.destination as i32 + delta).rem_euclid(len) as usize;
        let changed = next != self.destination;
        self.destination = next;
        changed
    }
}

impl Default for AddForm {
    fn default() -> Self {
        Self::new()
    }
}

/// What a pending deletion targets — captured when the confirm opens so a
/// later list refresh can't redirect the delete at the wrong row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteTarget {
    Rule(PermRuleRow),
    Dir(PermDirRow),
}

/// Inline yes/no confirmation for a deletion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteConfirm {
    /// `true` = Yes highlighted, `false` = No highlighted (default No, the
    /// safe choice — matches the agents-dialog destructive-action posture).
    pub yes: bool,
    pub target: DeleteTarget,
}

/// Where the cursor currently sits in the active tab.
pub enum Focused<'a> {
    /// The "Add a new rule…" / "Add directory…" sentinel at row 0.
    Add,
    Rule(&'a PermRuleRow),
    Dir(&'a PermDirRow),
    /// Empty list / out-of-range — no actionable row.
    None,
}

/// `/permissions` editor payload. Mutated by
/// `update/permissions_editor.rs`; rendered by
/// `presentation/permissions_editor::permissions_editor_content`.
#[derive(Debug, Clone)]
pub struct PermissionsEditorState {
    pub selected_tab: PermissionsEditorTab,
    pub allow_cursor: usize,
    pub ask_cursor: usize,
    pub deny_cursor: usize,
    pub workspace_cursor: usize,
    /// Every file-backed rule across all behaviors and sources.
    pub rules: Vec<PermRuleRow>,
    /// Additional directories; index 0 is the read-only cwd row.
    pub directories: Vec<PermDirRow>,
    pub cwd: String,
    /// Managed-policy lockdown — renders every row read-only and blocks
    /// add / delete.
    pub managed_only: bool,
    /// Active inline add form, or `None` for the list view.
    pub add_form: Option<AddForm>,
    /// Active deletion confirmation, or `None`.
    pub delete_confirm: Option<DeleteConfirm>,
}

impl PermissionsEditorState {
    /// Build editor state from the CLI snapshot. Prepends the read-only
    /// cwd row to the Workspace directory list.
    pub fn from_payload(payload: PermissionsEditorPayload) -> Self {
        let rules = payload
            .rules
            .into_iter()
            .map(|r| PermRuleRow {
                behavior: r.behavior,
                source: r.source,
                tool_pattern: r.tool_pattern,
                rule_content: r.rule_content,
            })
            .collect();

        let mut directories = Vec::with_capacity(payload.directories.len() + 1);
        directories.push(PermDirRow {
            path: payload.cwd.clone(),
            source: PermissionRuleSource::Session,
            is_cwd: true,
        });
        for dir in payload.directories {
            directories.push(PermDirRow {
                path: dir.path,
                source: dir.source,
                is_cwd: false,
            });
        }

        Self {
            selected_tab: PermissionsEditorTab::DEFAULT,
            allow_cursor: 0,
            ask_cursor: 0,
            deny_cursor: 0,
            workspace_cursor: 0,
            rules,
            directories,
            cwd: payload.cwd,
            managed_only: payload.managed_only,
            add_form: None,
            delete_confirm: None,
        }
    }

    /// Refresh the rule / directory data in place after a persisted edit,
    /// preserving the focused tab while clamping cursors and dropping any
    /// open form.
    pub fn refresh_from_payload(&mut self, payload: PermissionsEditorPayload) {
        let rebuilt = Self::from_payload(payload);
        self.rules = rebuilt.rules;
        self.directories = rebuilt.directories;
        self.cwd = rebuilt.cwd;
        self.managed_only = rebuilt.managed_only;
        self.add_form = None;
        self.delete_confirm = None;
        self.snap_cursors();
    }

    /// Rules of `behavior`, in payload order.
    pub fn rules_for(&self, behavior: PermissionBehavior) -> Vec<&PermRuleRow> {
        self.rules
            .iter()
            .filter(|r| r.behavior == behavior)
            .collect()
    }

    /// Number of selectable rows in the active tab, including the row-0
    /// "Add…" sentinel.
    pub fn active_len(&self) -> usize {
        match self.selected_tab.behavior() {
            Some(behavior) => 1 + self.rules_for(behavior).len(),
            None => 1 + self.directories.len(),
        }
    }

    pub fn active_cursor(&self) -> usize {
        match self.selected_tab {
            PermissionsEditorTab::Allow => self.allow_cursor,
            PermissionsEditorTab::Ask => self.ask_cursor,
            PermissionsEditorTab::Deny => self.deny_cursor,
            PermissionsEditorTab::Workspace => self.workspace_cursor,
        }
    }

    fn active_cursor_mut(&mut self) -> &mut usize {
        match self.selected_tab {
            PermissionsEditorTab::Allow => &mut self.allow_cursor,
            PermissionsEditorTab::Ask => &mut self.ask_cursor,
            PermissionsEditorTab::Deny => &mut self.deny_cursor,
            PermissionsEditorTab::Workspace => &mut self.workspace_cursor,
        }
    }

    /// Move the active tab's cursor by `delta`, clamped to the list (no
    /// wrap).
    pub fn nav(&mut self, delta: i32) -> bool {
        let len = self.active_len();
        let cursor = self.active_cursor_mut();
        if len == 0 {
            let changed = *cursor != 0;
            *cursor = 0;
            return changed;
        }
        let max = (len - 1) as i32;
        let next = (*cursor as i32 + delta).clamp(0, max) as usize;
        let changed = next != *cursor;
        *cursor = next;
        changed
    }

    /// Clamp every per-tab cursor into its list after a refresh.
    pub fn snap_cursors(&mut self) {
        let allow_max = self.rules_for(PermissionBehavior::Allow).len();
        let ask_max = self.rules_for(PermissionBehavior::Ask).len();
        let deny_max = self.rules_for(PermissionBehavior::Deny).len();
        let ws_max = self.directories.len();
        self.allow_cursor = self.allow_cursor.min(allow_max);
        self.ask_cursor = self.ask_cursor.min(ask_max);
        self.deny_cursor = self.deny_cursor.min(deny_max);
        self.workspace_cursor = self.workspace_cursor.min(ws_max);
    }

    /// Row under the cursor in the active tab.
    pub fn focused(&self) -> Focused<'_> {
        let cursor = self.active_cursor();
        if cursor == 0 {
            return Focused::Add;
        }
        let idx = cursor - 1;
        match self.selected_tab.behavior() {
            Some(behavior) => match self.rules_for(behavior).get(idx) {
                Some(rule) => Focused::Rule(rule),
                None => Focused::None,
            },
            None => match self.directories.get(idx) {
                Some(dir) => Focused::Dir(dir),
                None => Focused::None,
            },
        }
    }

    pub fn is_in_form(&self) -> bool {
        self.add_form.is_some() || self.delete_confirm.is_some()
    }
}

#[cfg(test)]
#[path = "permissions_editor.test.rs"]
mod tests;
