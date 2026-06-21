//! Skills dialog state — rows, locks, source precedence, and the
//! settings-file save diff.
//!
//! Split from `surface_payloads.rs`; the dialog interceptor still lives in
//! `update/skills_dialog.rs` until that surface migrates to `modal_pane/`.

/// `/skills` editable overlay state — flat list of [`SkillRow`]s
/// with filter + sort + selection state plus in-memory `pending`
/// override on each row.
#[derive(Debug, Clone)]
pub struct SkillsDialogState {
    /// All rows, stable insertion order (the renderer applies the
    /// current sort each frame; mutation order matters only for
    /// pending-state retention).
    pub rows: Vec<SkillRow>,
    /// Current filter query (lowercased on insert so the matcher
    /// can do byte-exact substring lookup). Empty = no filter.
    pub filter_query: String,
    /// Whether the inline filter input box is the active key
    /// target. `true` ⇒ printable characters append to the query;
    /// `false` ⇒ Space/Enter/Esc/`/`/`t` drive selection mode.
    pub filter_focused: bool,
    /// Whether the user toggled `t` to sort by descending token
    /// cost. Default (false) sorts by source-string lex + name.
    /// Not persisted — each `/skills` invocation starts at false.
    pub sort_by_tokens: bool,
    /// Index into the **filtered + sorted** view (not into
    /// [`Self::rows`]). The renderer recomputes the view each
    /// frame; this is clamped to `0..=view_len-1` on filter/sort
    /// change.
    pub selected_filtered_idx: usize,
    /// Bytes-per-token ratio for the token column. Comes from
    /// `SkillsDialogPayload.bytes_per_token`; the dialog divides
    /// [`SkillRow::frontmatter_bytes`] by this to render `~N tok`.
    pub bytes_per_token: i64,
}

/// One row in the editable `/skills` dialog. Carries everything
/// the renderer + save algorithm need — no round-trip to the
/// handler.
#[derive(Debug, Clone)]
pub struct SkillRow {
    pub name: String,
    pub source: SkillsDialogSource,
    /// Pre-built source label in lowercase for the filter matcher
    /// (`/` search hits name OR description OR source label).
    pub source_label_lower: String,
    pub plugin_name: Option<String>,
    pub frontmatter_bytes: i64,
    /// Lowercase haystack `name \u{1} description \u{1} source_label`
    /// — pre-computed so the filter matcher is one `contains` call
    /// per row.
    pub search_haystack: String,
    /// Value in `<cwd>/.coco/settings.local.json` right now.
    /// `None` ⇒ key absent.
    pub current_local: Option<SkillOverrideState>,
    /// Project-or-user resolution (without local / policy / flag).
    /// What the dialog reverts to when the user clears their local
    /// override.
    pub baseline: SkillOverrideState,
    /// User's in-memory pending edit. Initialized from
    /// `lock.forced_value` if locked, else from `current_local ??
    /// baseline`. Mutates on Space (lock rows are no-op).
    pub pending: SkillOverrideState,
    /// Optional lock — when set, the row renders `🔒 <label>`
    /// and refuses to cycle. The lock's `forced_value` is also
    /// surfaced as `pending` so save-diff never tries to persist
    /// a different value.
    pub lock: Option<SkillLock>,
}

/// TUI-side mirror of `coco_types::SkillsDialogSource`. Pinned to
/// the state crate so [`crate::state::ModalState`] doesn't import
/// `coco-types` directly for this field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SkillsDialogSource {
    BuiltIn,
    Project,
    User,
    Policy,
    Plugin,
    Mcp,
}

impl SkillsDialogSource {
    pub fn from_wire(s: coco_types::SkillsDialogSource) -> Self {
        match s {
            coco_types::SkillsDialogSource::BuiltIn => Self::BuiltIn,
            coco_types::SkillsDialogSource::Project => Self::Project,
            coco_types::SkillsDialogSource::User => Self::User,
            coco_types::SkillsDialogSource::Policy => Self::Policy,
            coco_types::SkillsDialogSource::Plugin => Self::Plugin,
            coco_types::SkillsDialogSource::Mcp => Self::Mcp,
        }
    }

    /// Lowercased label used by the inline source column and the
    /// filter haystack. Collapses `bundled`/`builtin` → `"built-in"`;
    /// the others use the snake-cased source name.
    pub fn label_lower(&self) -> &'static str {
        match self {
            Self::BuiltIn => "built-in",
            Self::Project => "project",
            Self::User => "user",
            Self::Policy => "policy",
            Self::Plugin => "plugin",
            Self::Mcp => "mcp",
        }
    }
}

/// Type alias for the wire skill-lock — keeps the state layer
/// free of `coco_types` imports outside this struct.
pub type SkillLock = coco_types::SkillLock;
pub use coco_types::SkillLockSource;
pub use coco_types::SkillOverrideState;

impl SkillsDialogState {
    /// Build from the wire payload. The renderer applies the
    /// sort (source-string lex + name; or token desc when
    /// `sort_by_tokens` is on) each frame, so we don't pre-sort.
    pub fn from_wire(payload: coco_types::SkillsDialogPayload) -> Self {
        let rows = payload
            .entries
            .into_iter()
            .map(|e| {
                let source = SkillsDialogSource::from_wire(e.source);
                let source_label_lower = source.label_lower().to_string();
                // pending starts at lock-forced-value when locked,
                // else current_local ?? baseline. The dialog never
                // surfaces a different `pending` on a locked row.
                let pending = e
                    .lock
                    .as_ref()
                    .map(|l| l.forced_value)
                    .or(e.current_local)
                    .unwrap_or(e.baseline);
                let mut haystack = String::with_capacity(
                    e.name.len() + e.description.len() + source_label_lower.len() + 2,
                );
                haystack.push_str(&e.name.to_lowercase());
                haystack.push('\u{1}');
                haystack.push_str(&e.description.to_lowercase());
                haystack.push('\u{1}');
                haystack.push_str(&source_label_lower);
                SkillRow {
                    name: e.name,
                    source,
                    source_label_lower,
                    plugin_name: e.plugin_name,
                    frontmatter_bytes: e.frontmatter_bytes,
                    search_haystack: haystack,
                    current_local: e.current_local,
                    baseline: e.baseline,
                    pending,
                    lock: e.lock,
                }
            })
            .collect();
        Self {
            rows,
            filter_query: String::new(),
            filter_focused: false,
            sort_by_tokens: false,
            selected_filtered_idx: 0,
            // Defensive fallback if a producer sets 0 — the
            // ~4-bytes/token English rule-of-thumb keeps the token
            // column non-zero.
            bytes_per_token: if payload.bytes_per_token > 0 {
                payload.bytes_per_token
            } else {
                4
            },
        }
    }

    /// Total entry count (drives the `{N} skills` subtitle).
    pub fn total(&self) -> usize {
        self.rows.len()
    }

    /// Whether any row carries a plugin source — drives the
    /// "Plugin skills are managed via /plugin" footer.
    pub fn has_plugin_rows(&self) -> bool {
        self.rows
            .iter()
            .any(|r| r.source == SkillsDialogSource::Plugin)
    }

    /// Indices into [`Self::rows`] for the currently-filtered +
    /// sorted view. Recomputed every call; the dialog renderer is
    /// expected to call this once per frame.
    pub fn filtered_view(&self) -> Vec<usize> {
        let mut indices: Vec<usize> = if self.filter_query.is_empty() {
            (0..self.rows.len()).collect()
        } else {
            self.rows
                .iter()
                .enumerate()
                .filter(|(_, r)| r.search_haystack.contains(&self.filter_query))
                .map(|(i, _)| i)
                .collect()
        };
        if self.sort_by_tokens {
            indices.sort_by(|a, b| {
                self.rows[*b]
                    .frontmatter_bytes
                    .cmp(&self.rows[*a].frontmatter_bytes)
                    .then_with(|| self.rows[*a].name.cmp(&self.rows[*b].name))
            });
        } else {
            indices.sort_by(|a, b| {
                self.rows[*a]
                    .source_label_lower
                    .cmp(&self.rows[*b].source_label_lower)
                    .then_with(|| self.rows[*a].name.cmp(&self.rows[*b].name))
            });
        }
        indices
    }

    /// Resolve the currently-focused row index in [`Self::rows`].
    /// Returns `None` when the filtered view is empty.
    pub fn focused_row(&self) -> Option<usize> {
        let view = self.filtered_view();
        view.get(self.selected_filtered_idx).copied()
    }

    /// Cycle the focused row's `pending` state through the 4-state
    /// ladder. **No-op on locked rows** — the cycle handler returns
    /// early before mutating state.
    pub fn cycle_focused(&mut self) {
        let Some(idx) = self.focused_row() else {
            return;
        };
        if self.rows[idx].lock.is_some() {
            return;
        }
        self.rows[idx].pending = self.rows[idx].pending.next();
    }

    /// Compute the diff to write to `localSettings.skill_overrides`.
    ///
    /// - For each row, compare `pending` to `baseline`. If equal,
    ///   write `null` (delete the local key); else write `pending`.
    /// - Skip the row entirely when `pending` already matches the
    ///   on-disk local value (no-op).
    /// - Locked rows are skipped (their `pending` is forced and
    ///   never written by the dialog).
    pub fn compute_save_diff(&self) -> SaveDiff {
        let mut diff = std::collections::BTreeMap::new();
        let mut total_edits = 0usize;
        for row in &self.rows {
            if row.lock.is_some() {
                continue;
            }
            let value_to_write: Option<SkillOverrideState> = if row.pending == row.baseline {
                None
            } else {
                Some(row.pending)
            };
            let effective_before = row.current_local.unwrap_or(row.baseline);
            if row.pending != effective_before {
                total_edits += 1;
            }
            if value_to_write != row.current_local {
                diff.insert(row.name.clone(), value_to_write);
            }
        }
        SaveDiff { diff, total_edits }
    }

    /// Apply a single printable character to the filter query.
    /// If the char is `/`, the literal slash is stripped (so typing `/`
    /// to enter filter mode doesn't push a literal `/` into the query).
    /// All other characters append.
    ///
    /// The caller should set `filter_focused = true` before calling
    /// this — the function itself only mutates the query string.
    pub fn apply_filter_char(&mut self, ch: char) {
        if ch == '/' {
            // Strip leading slash; if it's at the very start of an
            // empty query, this is the activation case and nothing
            // changes.
            return;
        }
        self.filter_query.push(ch.to_ascii_lowercase());
        self.clamp_selection();
    }

    /// Pop one character off the filter query. Returns whether
    /// the query was non-empty (keystroke is swallowed when the
    /// query is empty so the dialog stays in select mode).
    pub fn backspace_filter(&mut self) -> bool {
        if self.filter_query.is_empty() {
            return false;
        }
        self.filter_query.pop();
        self.clamp_selection();
        true
    }

    /// Clear the filter query and exit filter focus.
    pub fn clear_filter(&mut self) {
        self.filter_query.clear();
        self.filter_focused = false;
        self.clamp_selection();
    }

    /// Toggle source-vs-token-cost sort (bound to `t` key). Resets
    /// the selection index because the view order changed under it.
    pub fn toggle_sort(&mut self) {
        self.sort_by_tokens = !self.sort_by_tokens;
        self.selected_filtered_idx = 0;
    }

    /// Move selection up by one within the filtered view. No-op
    /// when at the top (the dialog does not wrap).
    pub fn move_up(&mut self) {
        if self.selected_filtered_idx > 0 {
            self.selected_filtered_idx -= 1;
        }
    }

    /// Move selection down by one within the filtered view.
    pub fn move_down(&mut self) {
        let view_len = self.filtered_view().len();
        if view_len == 0 {
            self.selected_filtered_idx = 0;
            return;
        }
        if self.selected_filtered_idx + 1 < view_len {
            self.selected_filtered_idx += 1;
        }
    }

    /// Clamp the selected index into the current view length so a
    /// filter change doesn't leave the cursor pointing past the
    /// last row.
    fn clamp_selection(&mut self) {
        let view_len = self.filtered_view().len();
        if view_len == 0 {
            self.selected_filtered_idx = 0;
            return;
        }
        if self.selected_filtered_idx >= view_len {
            self.selected_filtered_idx = view_len - 1;
        }
    }
}

/// Glyph + label table for the dialog's per-row state column.
///
/// Lives at the TUI state layer (not on `coco_types::SkillOverrideState`)
/// because the glyphs are a display concern — SDK consumers should
/// render their own table from the state enum.
pub fn skill_override_glyph_and_label(state: SkillOverrideState) -> (char, &'static str) {
    match state {
        SkillOverrideState::On => ('\u{2714}', "on"),
        SkillOverrideState::NameOnly => ('\u{2022}', "name-only"),
        SkillOverrideState::UserInvocableOnly => ('\u{25CB}', "user-only"),
        SkillOverrideState::Off => ('\u{2716}', "off"),
    }
}

/// Diff produced by [`SkillsDialogState::compute_save_diff`] —
/// directly serializable as the `skill_overrides` JSON patch the
/// SettingsWriter expects.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SaveDiff {
    /// Keys to update in `localSettings.skill_overrides`. `Some` ⇒
    /// write the new state. `None` ⇒ delete the key (deletion
    /// sentinel). Empty map ⇒ no-op save.
    pub diff: std::collections::BTreeMap<String, Option<SkillOverrideState>>,
    /// Number of rows whose effective state changed (different from
    /// what was effective at dialog-open time). Drives the toast:
    /// `Updated N override(s)` vs `No changes`.
    pub total_edits: usize,
}

impl SaveDiff {
    /// Whether any keys would change on disk.
    pub fn has_disk_changes(&self) -> bool {
        !self.diff.is_empty()
    }

    /// Render the diff as a `serde_json::Value` patch ready for
    /// [`coco_config::SettingsWriter::write_local`]. Each `None`
    /// becomes JSON `null` (the writer's delete sentinel).
    pub fn to_settings_patch(&self) -> serde_json::Value {
        let mut overrides = serde_json::Map::new();
        for (name, value) in &self.diff {
            let v = match value {
                Some(s) => serde_json::to_value(s).unwrap_or(serde_json::Value::Null),
                None => serde_json::Value::Null,
            };
            overrides.insert(name.clone(), v);
        }
        serde_json::json!({ "skill_overrides": serde_json::Value::Object(overrides) })
    }
}

#[cfg(test)]
#[path = "skills_dialog.test.rs"]
mod tests;
