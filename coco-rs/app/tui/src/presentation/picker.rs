//! Shared picker view models and renderer helpers.

#[cfg(test)]
use std::ops::Range;

use ratatui::prelude::*;

#[cfg(test)]
use super::layout;
use crate::i18n::t;
use crate::state::McpServerSelectState;
use crate::state::MemoryDialogRowKind;
use crate::state::MemoryDialogScope;
use crate::state::PluginDialogState;
use crate::state::PluginDialogTab;
use crate::state::SkillLockSource;
use crate::state::SkillOverrideState;
use crate::state::SkillRow;
use crate::state::SkillsDialogSource;
use crate::state::SkillsDialogState;
use crate::state::skills_dialog::skill_override_glyph_and_label;
use coco_tui_ui::style::UiStyles;

#[derive(Debug)]
#[cfg(test)]
pub(crate) enum PickerRow<'a, T> {
    Blank,
    Header(&'a str),
    Entry { filtered_index: usize, item: &'a T },
}

#[derive(Debug)]
#[cfg(test)]
pub(crate) struct PickerListView<'a, T> {
    pub(crate) rows: Vec<PickerRow<'a, T>>,
    pub(crate) visible: Range<usize>,
}

#[cfg(test)]
pub(crate) fn grouped_list<'a, 'b, T, F>(
    entries: &'b [&'a T],
    selected: Option<usize>,
    height: usize,
    group_label: F,
) -> PickerListView<'a, T>
where
    F: Fn(&'a T) -> &'a str,
{
    let mut rows = Vec::with_capacity(entries.len() + 8);
    let mut last_group: Option<&str> = None;
    for (filtered_index, entry) in entries.iter().copied().enumerate() {
        let group = group_label(entry);
        if last_group != Some(group) {
            if !rows.is_empty() {
                rows.push(PickerRow::Blank);
            }
            rows.push(PickerRow::Header(group));
            last_group = Some(group);
        }
        rows.push(PickerRow::Entry {
            filtered_index,
            item: entry,
        });
    }

    let selected_row = selected.and_then(|selected| {
        rows.iter().position(|row| {
            matches!(row, PickerRow::Entry { filtered_index, .. } if *filtered_index == selected)
        })
    });
    let visible = selected_row
        .map(|row| layout::visible_window(row, rows.len(), height))
        .unwrap_or(0..height.min(rows.len()));

    PickerListView { rows, visible }
}

#[cfg(test)]
pub(crate) trait SpanBgOpt<'a> {
    fn bg_opt(self, bg: Option<Color>) -> Span<'a>;
}

#[cfg(test)]
impl<'a> SpanBgOpt<'a> for Span<'a> {
    fn bg_opt(self, bg: Option<Color>) -> Span<'a> {
        if let Some(bg) = bg { self.bg(bg) } else { self }
    }
}

#[cfg(test)]
pub(crate) fn pad_line(mut line: Line<'static>, width: usize, bg: Option<Color>) -> Line<'static> {
    let used = line.width();
    if used < width {
        let pad = " ".repeat(width - used);
        let span = if let Some(bg) = bg {
            Span::raw(pad).bg(bg)
        } else {
            Span::raw(pad)
        };
        line.spans.push(span);
    }
    line
}

#[cfg(test)]
pub(crate) fn blank_line(width: usize) -> Line<'static> {
    Line::from(Span::raw(" ".repeat(width)))
}

#[cfg(test)]
pub(crate) fn collapse_hints(hints: &str, width: usize) -> String {
    let hints = hints.trim();
    if width == 0 {
        return String::new();
    }
    if layout::text_width(hints) <= width {
        return hints.to_string();
    }

    let mut collapsed = String::new();
    for part in hints.split("  ").filter(|part| !part.is_empty()) {
        let candidate = if collapsed.is_empty() {
            part.to_string()
        } else {
            format!("{collapsed}  {part}")
        };
        if layout::text_width(&candidate) > width {
            break;
        }
        collapsed = candidate;
    }

    if collapsed.is_empty() {
        layout::truncate_to_width(hints, width)
    } else {
        collapsed
    }
}

pub(crate) fn first_line_preview(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or("");
    let mut out = String::new();
    for (width, ch) in line.chars().enumerate() {
        if width + 1 > max {
            out.push('\u{2026}');
            break;
        }
        out.push(ch);
    }
    out
}

/// Render the editable `/skills` overlay. Flat list, per-row 4-state
/// override cycle, inline source label, lock annotation, filter
/// input + sort toggle.
pub(crate) fn skills_dialog_content(
    s: &SkillsDialogState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    let title = t!("dialog.title_skills").to_string();

    if s.rows.is_empty() {
        return (
            title,
            t!("dialog.skills_empty").to_string(),
            styles.primary(),
        );
    }

    let view = s.filtered_view();
    let mut body = String::new();

    // Subtitle: `<filtered>/<total> skills` when filter active, else
    // `<total> skills`. Drives the visible "20 skills" header.
    let total = s.rows.len();
    let total_noun = if total == 1 {
        t!("dialog.skills_noun_singular")
    } else {
        t!("dialog.skills_noun_plural")
    };
    if s.filter_query.is_empty() {
        body.push_str(&t!(
            "dialog.skills_subtitle",
            count = total.to_string().as_str(),
            noun = total_noun.as_ref()
        ));
    } else {
        body.push_str(&format!("{}/{} {}", view.len(), total, total_noun.as_ref()));
    }
    body.push_str(" · ");
    body.push_str(&hint_line(s));
    body.push('\n');

    // Filter input row — placeholder "Search skills…". Render the query
    // in-line; downstream styling is applied by the higher-level surface
    // renderer.
    body.push('\n');
    body.push_str("⌕ ");
    if s.filter_query.is_empty() {
        body.push_str(&t!("dialog.skills_filter_placeholder"));
    } else {
        body.push_str(&s.filter_query);
    }
    body.push('\n');

    if view.is_empty() {
        body.push('\n');
        body.push_str(&t!(
            "dialog.skills_empty_filter",
            query = s.filter_query.as_str()
        ));
    } else {
        for (i, row_idx) in view.iter().enumerate() {
            body.push('\n');
            body.push_str(&render_skill_row(
                &s.rows[*row_idx],
                i == s.selected_filtered_idx,
                s.bytes_per_token,
            ));
        }
    }

    // Plugin footer: only rendered when at least one plugin row is present.
    if s.has_plugin_rows() {
        body.push_str("\n\n");
        body.push_str(&t!("dialog.skills_plugin_footer"));
    }

    (title, body, styles.primary())
}

pub(crate) fn plugin_dialog_content(
    p: &PluginDialogState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    let mut body = String::new();
    body.push_str(&render_plugin_tabs(p));
    body.push_str(" · ↑/↓ move · Tab switch · / search · Enter action · Esc close\n\n");
    body.push_str("⌕ ");
    if p.filter_query.is_empty() {
        body.push_str("Search plugins, marketplaces, errors");
    } else {
        body.push_str(&p.filter_query);
    }
    body.push('\n');

    match p.selected_tab {
        PluginDialogTab::Installed => render_installed_tab(p, &mut body),
        PluginDialogTab::Marketplaces => render_marketplace_tab(p, &mut body),
        PluginDialogTab::Errors => render_error_tab(p, &mut body),
    }

    ("Plugins".to_string(), body, styles.primary())
}

fn render_plugin_tabs(p: &PluginDialogState) -> String {
    PluginDialogTab::ALL
        .iter()
        .map(|tab| {
            let count = match tab {
                PluginDialogTab::Installed => p.installed.len(),
                PluginDialogTab::Marketplaces => p.marketplaces.len(),
                PluginDialogTab::Errors => p.errors.len(),
            };
            if *tab == p.selected_tab {
                format!("[{} {count}]", tab.label())
            } else {
                format!("{} {count}", tab.label())
            }
        })
        .collect::<Vec<_>>()
        .join("  ")
}

fn render_installed_tab(p: &PluginDialogState, body: &mut String) {
    let view = p.filtered_installed_indices();
    if view.is_empty() {
        body.push_str("\nNo installed plugins match.\n");
        return;
    }
    for (i, idx) in view.iter().enumerate() {
        let row = &p.installed[*idx];
        let cursor = if i == p.selected_idx { ">" } else { " " };
        let state = if row.blocked_by_policy {
            "blocked"
        } else if row.enabled {
            "enabled"
        } else {
            "disabled"
        };
        let version = row
            .version
            .as_deref()
            .map(|v| format!(" v{v}"))
            .unwrap_or_default();
        body.push_str(&format!(
            "\n{cursor} {}{version} · {state} · {}",
            row.id, row.source
        ));
    }
    if let Some(idx) = view.get(p.selected_idx)
        && let Some(row) = p.installed.get(*idx)
    {
        body.push_str("\n\n");
        body.push_str(&format!("{}\n", row.name));
        if let Some(desc) = &row.description {
            body.push_str(desc);
            body.push('\n');
        }
        body.push_str(&format!("Path: {}\n", row.path));
        if !row.options.is_empty() {
            body.push_str("\nOptions\n");
            for option in &row.options {
                let required = if option.required {
                    "required"
                } else {
                    "optional"
                };
                let current = option
                    .current_value
                    .as_ref()
                    .map_or("unset".to_string(), serde_json::Value::to_string);
                body.push_str(&format!(
                    "  {} ({}, {}) = {}\n",
                    option.key, option.value_type, required, current
                ));
            }
        }
        if !row.mcp_servers.is_empty() {
            body.push_str("\nMCP servers\n");
            for server in &row.mcp_servers {
                let state = if server.enabled {
                    "enabled"
                } else {
                    "disabled"
                };
                let cfg = if server.needs_config {
                    ", needs config"
                } else {
                    ""
                };
                body.push_str(&format!("  {} · {state}{cfg}\n", server.display_name));
                for tool in &server.tools {
                    let desc = tool.description.as_deref().unwrap_or("");
                    body.push_str(&format!("    - {} {}\n", tool.name, desc));
                }
            }
        }
        if let Some(action) = row.actions.first() {
            body.push_str(&format!("\nEnter: {}", action.label));
        }
    }
}

fn render_marketplace_tab(p: &PluginDialogState, body: &mut String) {
    let view = p.filtered_marketplace_indices();
    if view.is_empty() {
        body.push_str("\nNo marketplaces match.\n");
        return;
    }
    for (i, idx) in view.iter().enumerate() {
        let row = &p.marketplaces[*idx];
        let cursor = if i == p.selected_idx { ">" } else { " " };
        let official = if row.official { "official" } else { "custom" };
        body.push_str(&format!(
            "\n{cursor} {} · {} plugins · {official}",
            row.name, row.plugin_count
        ));
        if let Some(source) = &row.source {
            body.push_str(&format!(" · {source}"));
        }
    }
    if let Some(idx) = view.get(p.selected_idx)
        && let Some(row) = p.marketplaces.get(*idx)
        && let Some(action) = row.actions.first()
    {
        body.push_str(&format!("\n\nEnter: {}", action.label));
    }
}

fn render_error_tab(p: &PluginDialogState, body: &mut String) {
    let view = p.filtered_error_indices();
    if view.is_empty() {
        body.push_str("\nNo plugin load errors.\n");
        return;
    }
    for (i, idx) in view.iter().enumerate() {
        let row = &p.errors[*idx];
        let cursor = if i == p.selected_idx { ">" } else { " " };
        body.push_str(&format!("\n{cursor} {} · {}", row.plugin_id, row.message));
    }
}

/// Format the "Space to cycle, Enter to save, …" hint line. Two
/// variants: select mode shows the full ladder; filter-focused mode
/// swaps in the filter instructions.
fn hint_line(s: &SkillsDialogState) -> String {
    if s.filter_focused {
        return t!("dialog.skills_hint_filter_focused").to_string();
    }
    t!("dialog.skills_hint_select").to_string()
}

/// One skill row in the dialog:
///
/// ```text
///   ✓ on        | my-skill · user · 42 tok
///   🔒 on       | claude-api · built-in · 30 tok · locked by author
/// ```
fn render_skill_row(row: &SkillRow, focused: bool, bytes_per_token: i64) -> String {
    let (glyph, label) = row
        .lock
        .as_ref()
        .map(|l| ('\u{1F512}', state_label_for_lock(l.forced_value)))
        .unwrap_or_else(|| skill_override_glyph_and_label(row.pending));
    let cursor = if focused { '\u{276F}' } else { ' ' }; // ❯
    let tokens = if bytes_per_token > 0 {
        row.frontmatter_bytes / bytes_per_token
    } else {
        row.frontmatter_bytes / 4
    };
    let mut line = format!("{cursor} {glyph} {label:<9} {}", row.name);
    line.push_str(" · ");
    line.push_str(skills_source_label(row.source));
    if let Some(plugin) = &row.plugin_name {
        line.push_str(" · ");
        line.push_str(plugin);
    }
    line.push_str(" · ");
    line.push_str(&t!(
        "dialog.skills_token_suffix",
        tokens = tokens.to_string().as_str()
    ));
    if let Some(lock) = &row.lock {
        line.push_str(" · ");
        line.push_str(&t!(
            "dialog.skills_locked_by",
            source = lock_source_label(lock.source)
        ));
    }
    line
}

fn state_label_for_lock(state: SkillOverrideState) -> &'static str {
    let (_, label) = skill_override_glyph_and_label(state);
    label
}

fn skills_source_label(source: SkillsDialogSource) -> &'static str {
    source.label_lower()
}

fn lock_source_label(source: SkillLockSource) -> &'static str {
    match source {
        SkillLockSource::Policy => "policy",
        SkillLockSource::Flag => "flag",
        SkillLockSource::Author => "author",
        SkillLockSource::Plugin => "plugin",
    }
}

pub(crate) fn mcp_server_select_content(
    ms: &McpServerSelectState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    let items: Vec<String> = ms
        .servers
        .iter()
        .map(|s| {
            let check = if s.selected { "[x]" } else { "[ ]" };
            format!(
                "  {check} {} ({})",
                s.name,
                t!("mcp.tools_count", count = s.tool_count)
            )
        })
        .collect();

    (
        t!("dialog.title_select_mcp_servers").to_string(),
        format!(
            "{}\n\n{}",
            t!("dialog.filter_prefix", text = ms.filter.as_str()),
            items.join("\n")
        ),
        styles.accent(),
    )
}

pub(crate) fn memory_scope_tag(scope: MemoryDialogScope) -> &'static str {
    match scope {
        MemoryDialogScope::Managed => "[managed]",
        MemoryDialogScope::User => "[user]",
        MemoryDialogScope::Project => "[project]",
        MemoryDialogScope::ProjectLocal => "[project-local]",
        MemoryDialogScope::ProjectConfig => "[project-config]",
        MemoryDialogScope::Subdir => "[subdir]",
        MemoryDialogScope::Imported => "[@-imported]",
        MemoryDialogScope::AutoMemFolder => "[auto-mem]",
        MemoryDialogScope::TeamMemFolder => "[team-mem]",
        MemoryDialogScope::AgentMemFolder => "[agent-mem]",
    }
}

pub(crate) fn memory_row_kind_tag(kind: MemoryDialogRowKind) -> &'static str {
    match kind {
        MemoryDialogRowKind::File {
            exists: true,
            read_only: false,
        } => "[file:exists]",
        MemoryDialogRowKind::File {
            exists: false,
            read_only: false,
        } => "[file:new]",
        MemoryDialogRowKind::File {
            read_only: true, ..
        } => "[file:read-only]",
        MemoryDialogRowKind::Folder { enabled: true } => "[folder:on]",
        MemoryDialogRowKind::Folder { enabled: false } => "[folder:off]",
        MemoryDialogRowKind::Toggle { enabled: true } => "[toggle:on]",
        MemoryDialogRowKind::Toggle { enabled: false } => "[toggle:off]",
    }
}

/// Re-export of the dedicated `/agents` overlay renderer. The
/// canonical implementation lives in
/// [`super::agents_dialog::agents_dialog_content`]; surfacing it from
/// `picker::` keeps the `surface_content/pickers.rs` delegate using
/// the same one-step indirection style as `skills_dialog_content` /
/// `memory_dialog_content`.
pub(crate) use super::agents_dialog::agents_dialog_content;

#[cfg(test)]
#[path = "picker.test.rs"]
mod tests;
