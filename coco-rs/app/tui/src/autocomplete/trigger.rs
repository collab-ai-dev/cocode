//! Autocomplete trigger detection and suggestion-state refresh.
//!
//! Responsibilities:
//! - [`detect`] — pure function, `(text, cursor)` → `Option<Trigger>`.
//! - [`refresh_suggestions`] — mutates `UiState::completion.active` based
//!   on the current input. The unified `@` path seeds the popup with
//!   synchronous agent matches (from `session.available_agents`); the
//!   async file-search manager fills in path matches as they arrive and
//!   the merger interleaves them by score.
//!
//! Removed: the legacy `@agent-<name>` sub-prefix. Agents are pulled in
//! by fuzzy-matching the bare `@query`; `core/context::extract_mentions`
//! accepts either `agent-` prefix or `(agent)` suffix on submit, so popup
//! insertion uses the suffix form.

use crate::state::ActiveSuggestions;
use crate::state::AppState;
use crate::state::InlineGhost;
use crate::state::SuggestionKind;
use crate::widgets::suggestion_popup::SuggestionItem;
use coco_types::CommandArgumentKind;

/// A detected trigger in the input buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trigger {
    pub kind: SuggestionKind,
    /// Byte offset of the trigger start (the `/` or `@`).
    pub pos: usize,
    /// Text the user typed after the trigger (filter query).
    pub query: String,
}

/// Scan `text` up to `cursor` (byte offset) for an active autocomplete trigger.
///
/// Rules:
/// - Leading `/` at position 0 with no space yet → `SlashCommand`. The
///   query is everything after the `/` up to cursor.
/// - `@#word` → `Symbol` (LSP symbol search).
/// - `@word` (anything else after `@`) → `At` (unified popup: agents +
///   file paths + MCP resources).
///
/// `@`-mentions must be at text start or immediately after whitespace so
/// emails like `a@b.com` or tags already-accepted don't re-trigger. Both
/// `cursor` and the returned `Trigger.pos` are byte offsets, so CJK / wide
/// characters before the trigger don't shift the splice site.
pub fn detect(text: &str, cursor: usize) -> Option<Trigger> {
    let cursor = cursor.min(text.len());
    let prefix = &text[..cursor];

    // Slash command: `/` at byte 0, no whitespace before cursor.
    if prefix.starts_with('/') && !prefix[1..].chars().any(char::is_whitespace) {
        return Some(Trigger {
            kind: SuggestionKind::SlashCommand,
            pos: 0,
            query: prefix[1..].to_string(),
        });
    }

    if let Some(token) = current_token(prefix, cursor)
        && token.text.starts_with('@')
    {
        let tail = &token.text[1..];
        let (kind, query) = classify_at_trigger(tail);
        return Some(Trigger {
            kind,
            pos: token.start,
            query,
        });
    }

    None
}

fn classify_at_trigger(tail: &str) -> (SuggestionKind, String) {
    if let Some(rest) = tail.strip_prefix('#') {
        return (SuggestionKind::Symbol, rest.to_string());
    }
    (SuggestionKind::At, tail.to_string())
}

/// Recompute `ui.completion.active` from the current input buffer.
///
/// Called after any input mutation (InsertChar, DeleteBackward, Yank, etc.).
/// Dismisses suggestions when no trigger is detected; installs or refreshes
/// them when one is. Synchronous sources (slash commands, agents) populate
/// items inline; async sources leave `items` empty pending a search result.
pub fn refresh_suggestions(state: &mut AppState) {
    let text = state.ui.input.text().to_string();
    let cursor = state.ui.input.textarea.cursor();
    let trigger =
        detect_command_directory_trigger(state, &text, cursor).or_else(|| detect(&text, cursor));
    let Some(mut trigger) = trigger else {
        state.ui.completion.clear_active();
        state.ui.sync_popup_from_active_suggestions();
        refresh_inline_ghost(state);
        return;
    };
    if trigger.kind == SuggestionKind::At && is_path_like_query(&trigger.query) {
        trigger.kind = SuggestionKind::Path;
    }
    let token_range = trigger.pos.min(cursor)..cursor;
    let token_text = text
        .get(token_range.clone())
        .unwrap_or_default()
        .to_string();
    let request_key = crate::completion::CompletionRequestKey {
        kind: trigger.kind,
        token_range: token_range.clone(),
        token_text,
        query: trigger.query.clone(),
        generation: 0,
    };
    if state.ui.completion.is_dismissed(&request_key) {
        state.ui.completion.clear_active();
        state.ui.sync_popup_from_active_suggestions();
        refresh_inline_ghost(state);
        return;
    }
    state.ui.completion.dismissed = None;

    // Slash is fully synchronous. The unified `@` path seeds the popup
    // with agent matches (synchronous, from `session.available_agents`)
    // and leaves room for the async FileSearchManager to merge in path
    // matches as results arrive via `apply_async_result`. Symbol is
    // entirely async — empty until LSP results land.
    //
    // The Autocomplete keybinding context only activates when items is
    // non-empty, so arrow keys keep passing through to input editing until
    // results materialize.
    let mut items = match trigger.kind {
        SuggestionKind::SlashCommand => slash_items(state, &trigger.query),
        SuggestionKind::At => unified_seed_items(state, &trigger.query),
        SuggestionKind::Path | SuggestionKind::Directory => Vec::new(),
        SuggestionKind::CustomTitle => resume_items(state, &trigger.query),
        SuggestionKind::Symbol => Vec::new(),
    };

    // Snapshot the prior popup for the SAME trigger token (kind + position):
    // its selection (so navigation stays stable as the user types) and its
    // rows (so async-backed popups don't blank between keystrokes).
    let (prior_selected, prior_items) = state
        .ui
        .completion
        .active
        .as_ref()
        .filter(|s| s.kind == trigger.kind && s.trigger_pos == trigger.pos)
        .map(|s| (s.selected, s.items.clone()))
        .unwrap_or((0, Vec::new()));

    // Async-backed kinds fill `items` only when the debounced result lands
    // (At merges file results, Path/Directory come from FileSearchManager,
    // Symbol from LSP). Installing an empty Vec on every keystroke blanks the
    // popup for ~one debounce interval — the severe flicker while typing a
    // path. Keep the previously-shown rows mounted until the async result for
    // the new key replaces them in `apply_async_result_for_key`; a genuinely
    // empty *final* result still collapses the popup there. Fully-synchronous
    // kinds (SlashCommand, CustomTitle) keep clearing immediately — an empty
    // result is authoritative for them.
    if items.is_empty()
        && !prior_items.is_empty()
        && matches!(
            trigger.kind,
            SuggestionKind::At
                | SuggestionKind::Path
                | SuggestionKind::Directory
                | SuggestionKind::Symbol
        )
    {
        items = prior_items;
    }

    // Clamp the preserved selection to the new item range.
    let selected = if items.is_empty() {
        0
    } else {
        prior_selected.min(items.len() - 1)
    };

    state.ui.completion.set_active(
        ActiveSuggestions {
            kind: trigger.kind,
            items,
            selected,
            query: trigger.query,
            trigger_pos: trigger.pos,
        },
        token_range,
        request_key.token_text,
    );
    state.ui.sync_popup_from_active_suggestions();
    refresh_inline_ghost(state);
}

/// Apply an async search result (from FileSearchManager or
/// SymbolSearchManager) to the active suggestion popup.
///
/// For the unified `@` popup, file results are merged with the seeded agent
/// items via [`super::unified::merge_file_results`]: agents stay first,
/// files appended after, cap 15.
///
/// Returns `true` when the result was adopted. Drops stale results silently
/// when the user has already moved to a different trigger kind, query, or
/// dismissed the popup altogether — the manager's debounced cancel isn't
/// instant, so this guards against a slow result clobbering a newer query.
pub fn apply_async_result(
    state: &mut AppState,
    kind: SuggestionKind,
    query: &str,
    suggestions: Vec<SuggestionItem>,
) -> bool {
    let Some(key) = state.ui.completion.active_key.clone() else {
        return false;
    };
    if key.kind != kind || key.query != query {
        return false;
    }
    apply_async_result_for_key(state, &key, suggestions)
}

pub fn apply_async_result_for_key(
    state: &mut AppState,
    key: &crate::completion::CompletionRequestKey,
    suggestions: Vec<SuggestionItem>,
) -> bool {
    if !state.ui.completion.active_key_matches(key) {
        return false;
    }
    // Snapshot agent matches BEFORE we take a mutable borrow on
    // `completion.active` — splitting `state.session` (immutable, agents)
    // and `state.ui.completion.active` (mutable) requires the immutable
    // half to be consumed first under NLL.
    let seed = if key.kind == SuggestionKind::At {
        unified_seed_items(state, &key.query)
    } else {
        Vec::new()
    };

    let Some(sug) = state.ui.completion.active.as_mut() else {
        return false;
    };
    if sug.kind != key.kind || sug.query != key.query {
        return false;
    }
    sug.items = match key.kind {
        SuggestionKind::At => super::unified::merge_file_results(seed, suggestions),
        _ => suggestions,
    };
    sug.selected = if sug.items.is_empty() {
        0
    } else {
        sug.selected.min(sug.items.len() - 1)
    };
    state.ui.sync_popup_from_active_suggestions();
    true
}

fn slash_items(state: &AppState, query: &str) -> Vec<SuggestionItem> {
    // Delegate ranking to the dedicated module; keeping the call shallow
    // here lets the trigger module stay focused on detection vs. matching.
    super::slash::rank(query, &state.session.available_commands)
}

fn unified_seed_items(state: &AppState, query: &str) -> Vec<SuggestionItem> {
    let agents = super::unified::seed_agent_items(&state.session.available_agents, query);
    let resources =
        super::unified::seed_mcp_resource_items(&state.session.available_mcp_resources, query);
    super::unified::merge_seeded_provider_items(agents, resources)
}

fn resume_items(state: &AppState, query: &str) -> Vec<SuggestionItem> {
    let query = query.trim();
    let query_lower = query.to_lowercase();
    state
        .session
        .saved_sessions
        .iter()
        .filter(|session| {
            query_lower.is_empty()
                || session.id.to_lowercase().contains(&query_lower)
                || session.label.to_lowercase().contains(&query_lower)
        })
        .take(15)
        .map(|session| SuggestionItem {
            label: session.id.clone(),
            description: Some(session.label.clone()),
            metadata: Some(crate::widgets::suggestion_popup::SuggestionMeta::Session),
        })
        .collect()
}

fn refresh_inline_ghost(state: &mut AppState) {
    state.ui.input.clear_inline_ghost();
    if state
        .ui
        .completion
        .active
        .as_ref()
        .is_some_and(|s| !s.items.is_empty())
    {
        return;
    }

    let text = state.ui.input.text();
    let cursor = state.ui.input.textarea.cursor();
    if let Some(ghost) = mid_input_slash_ghost(text, cursor, state) {
        state.ui.input.set_inline_ghost(ghost);
        return;
    }
    if let Some(ghost) = shell_history_ghost(text, cursor, state) {
        state.ui.input.set_inline_ghost(ghost);
    }
}

fn mid_input_slash_ghost(text: &str, cursor: usize, state: &AppState) -> Option<InlineGhost> {
    let token = current_token(text, cursor)?;
    if token.start == 0 || !token.text.starts_with('/') {
        return None;
    }
    let query = &token.text[1..];
    if query.is_empty() {
        return None;
    }
    let item = slash_items(state, query).into_iter().next()?;
    let typed = &text[token.start..cursor];
    if !item.label.starts_with(typed) || item.label.len() <= typed.len() {
        return None;
    }
    let suffix = item.label[typed.len()..].to_string();
    Some(InlineGhost {
        text: suffix.clone(),
        insert_position: cursor,
        replace_start: cursor,
        replace_end: cursor,
        replacement: suffix,
        cursor_after_accept: item.label.len() - typed.len() + cursor,
    })
}

fn shell_history_ghost(text: &str, cursor: usize, state: &AppState) -> Option<InlineGhost> {
    if cursor != text.len() || !text.starts_with('!') {
        return None;
    }
    state
        .ui
        .input
        .history
        .iter()
        .map(|entry| entry.text.as_str())
        .find(|entry| entry.starts_with(text) && entry.len() > text.len())
        .map(|entry| {
            let suffix = entry[text.len()..].to_string();
            InlineGhost {
                text: suffix.clone(),
                insert_position: cursor,
                replace_start: cursor,
                replace_end: cursor,
                replacement: suffix,
                cursor_after_accept: entry.len(),
            }
        })
}

#[derive(Debug, Clone, Copy)]
struct CurrentToken<'a> {
    start: usize,
    text: &'a str,
}

fn current_token(text: &str, cursor: usize) -> Option<CurrentToken<'_>> {
    let cursor = cursor.min(text.len());
    let prefix = &text[..cursor];
    let mut start = None;
    let mut in_quote = false;
    let mut escape = false;
    for (i, ch) in prefix.char_indices() {
        if start.is_none() {
            if ch.is_whitespace() {
                continue;
            }
            start = Some(i);
        }
        if escape {
            escape = false;
            continue;
        }
        match ch {
            '\\' if in_quote => escape = true,
            '"' => in_quote = !in_quote,
            _ if ch.is_whitespace() && !in_quote => start = None,
            _ => {}
        }
    }
    let start = start.unwrap_or(cursor);
    (start < cursor).then_some(CurrentToken {
        start,
        text: &text[start..cursor],
    })
}

fn detect_command_directory_trigger(
    state: &AppState,
    text: &str,
    cursor: usize,
) -> Option<Trigger> {
    let cursor = cursor.min(text.len());
    let prefix = &text[..cursor];
    let stripped = prefix.strip_prefix('/')?;
    let (name, args_with_space) = stripped.split_once(char::is_whitespace)?;
    let command = state
        .session
        .available_commands
        .iter()
        .find(|cmd| cmd.name == name)?;
    let args_start = cursor - args_with_space.len();
    let token = current_token(prefix, cursor).unwrap_or(CurrentToken {
        start: cursor,
        text: "",
    });
    if token.start < args_start {
        return None;
    }
    match command.argument_kind {
        CommandArgumentKind::FilePath if is_path_like_query(token.text) => Some(Trigger {
            kind: SuggestionKind::Path,
            pos: token.start,
            query: token.text.to_string(),
        }),
        CommandArgumentKind::DirectoryPath if is_path_like_query(token.text) => Some(Trigger {
            kind: SuggestionKind::Directory,
            pos: token.start,
            query: token.text.to_string(),
        }),
        CommandArgumentKind::SessionId => Some(Trigger {
            kind: SuggestionKind::CustomTitle,
            pos: token.start,
            query: token.text.to_string(),
        }),
        _ => None,
    }
}

fn is_path_like_query(query: &str) -> bool {
    let query = query.strip_prefix('"').unwrap_or(query);
    query.starts_with("~/")
        || query.starts_with("./")
        || query.starts_with("../")
        || query.starts_with('/')
}

#[cfg(test)]
#[path = "trigger.test.rs"]
mod tests;
