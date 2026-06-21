//! Completion state helpers and async providers.
//!
//! This module owns the pieces that must not run on the input hot path.

use std::ops::Range;

use coco_types::CommandTypeTag;

use crate::state::AppState;
use crate::widgets::suggestion_popup::SuggestionItem;
use crate::widgets::suggestion_popup::SuggestionMeta;

pub mod path_provider;

pub use path_provider::PathCompletionEvent;
pub use path_provider::PathCompletionManager;
pub use path_provider::create_path_completion_channel;

/// How a keypress should consume the active completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceptMode {
    /// Tab: extend a shared path prefix when one exists, otherwise accept the
    /// selected row.
    ExtendCommonPrefix,
    /// Enter: accept the selected row without applying shared path prefixes.
    AcceptSelected,
    /// Enter in a submit context: same as selected accept, except directory
    /// argument popups submit the command as typed.
    SubmitSelected,
}

/// Result of applying a completion to the editable input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionInsertion {
    pub replacement: String,
    pub cursor_position: usize,
    pub keep_popup: bool,
    pub should_submit: bool,
}

/// Which autocomplete trigger produced the active suggestions.
///
/// Determines the popup title, the source of suggestion items, and how the
/// accepted item is substituted back into the input. Kinds map to TS's
/// unified `@` mention trigger plus the leading `/` slash-command trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SuggestionKind {
    /// Leading `/` — populated from `session.available_commands`.
    SlashCommand,
    /// `@query` — unified popup combining agents (synchronous, from
    /// `session.available_agents`), file paths (async via
    /// `FileSearchManager`), and MCP resources. Per-row kind is carried
    /// on `SuggestionItem::metadata`.
    At,
    /// Explicit path completion after `@~/`, `@./`, `@../`, or `@/`.
    /// Kept distinct from fuzzy `At` so async file-index results cannot
    /// replace path-provider rows for the same visible token.
    Path,
    /// Path completion inside a slash-command argument position where
    /// directories should keep the popup open and Enter should submit
    /// the already-typed command instead of forcing the selected row.
    Directory,
    /// Saved-session title/id completion for `/resume`.
    CustomTitle,
    /// `@#symbol` — populated asynchronously by `SymbolSearchManager` (LSP).
    Symbol,
}

/// Active autocomplete session: popup rendered above input, intercepts
/// `Up/Down/Tab/Esc` while letting regular typing pass through.
#[derive(Debug, Clone)]
pub struct ActiveSuggestions {
    pub kind: SuggestionKind,
    /// Items to show in the popup — filtered by `query` before display.
    pub items: Vec<SuggestionItem>,
    /// Currently selected index into the filtered list.
    pub selected: usize,
    /// The filter text the user has typed after the trigger.
    pub query: String,
    /// Byte offset in `input.text` where the trigger started (the `/`
    /// or `@`). Used when accepting a suggestion to splice the selection
    /// back into the input via `textarea.replace_range`.
    pub trigger_pos: usize,
}

/// Typed MCP resource row available to the unified `@` completion provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpResourceCompletion {
    pub server: String,
    pub uri: String,
    pub name: String,
    pub description: Option<String>,
}

/// Typed Slack channel row. The default source is empty; callers must wire a
/// real channel list before any Slack completion can return rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackChannelCompletion {
    pub name: String,
    pub description: Option<String>,
}

/// Canonical identity for one completion request.
///
/// Async providers echo this key back with their results. The TUI applies
/// a result only when it exactly matches the active key, so repeated text in
/// different tokens and stale results from previous generations cannot cross.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CompletionRequestKey {
    pub kind: SuggestionKind,
    pub token_range: Range<usize>,
    pub token_text: String,
    pub query: String,
    pub generation: u64,
}

impl CompletionRequestKey {
    fn same_request_without_generation(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.token_range == other.token_range
            && self.token_text == other.token_text
            && self.query == other.query
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DismissedCompletion {
    pub kind: SuggestionKind,
    pub token_range: Range<usize>,
    pub token_text: String,
}

impl DismissedCompletion {
    pub fn from_key(key: &CompletionRequestKey) -> Self {
        Self {
            kind: key.kind,
            token_range: key.token_range.clone(),
            token_text: key.token_text.clone(),
        }
    }

    pub fn matches_key(&self, key: &CompletionRequestKey) -> bool {
        self.kind == key.kind
            && self.token_range == key.token_range
            && self.token_text == key.token_text
    }
}

/// UI-local completion state and request-key bookkeeping.
#[derive(Debug, Default)]
pub struct CompletionState {
    pub active: Option<ActiveSuggestions>,
    pub active_key: Option<CompletionRequestKey>,
    pub dismissed: Option<DismissedCompletion>,
    pub last_dispatched: Option<CompletionRequestKey>,
    generation: u64,
}

impl CompletionState {
    pub fn set_active(
        &mut self,
        suggestions: ActiveSuggestions,
        token_range: Range<usize>,
        token_text: String,
    ) -> CompletionRequestKey {
        let candidate = CompletionRequestKey {
            kind: suggestions.kind,
            token_range,
            token_text,
            query: suggestions.query.clone(),
            generation: self.generation,
        };
        let generation = self
            .active_key
            .as_ref()
            .filter(|key| key.same_request_without_generation(&candidate))
            .map(|key| key.generation)
            .unwrap_or_else(|| {
                self.generation = self.generation.wrapping_add(1);
                self.generation
            });
        let key = CompletionRequestKey {
            generation,
            ..candidate
        };
        self.active = Some(suggestions);
        self.active_key = Some(key.clone());
        key
    }

    pub fn clear_active(&mut self) {
        self.active = None;
        self.active_key = None;
    }

    pub fn clear_all(&mut self) {
        self.clear_active();
        self.dismissed = None;
        self.last_dispatched = None;
    }

    pub fn dismiss_active(&mut self) {
        if let Some(key) = self.active_key.as_ref() {
            self.dismissed = Some(DismissedCompletion::from_key(key));
        }
        self.clear_active();
    }

    pub fn is_dismissed(&self, key: &CompletionRequestKey) -> bool {
        self.dismissed
            .as_ref()
            .is_some_and(|dismissed| dismissed.matches_key(key))
    }

    pub fn active_key_matches(&self, key: &CompletionRequestKey) -> bool {
        self.active_key.as_ref() == Some(key)
    }
}

#[derive(Debug, Clone)]
struct AcceptedSlashCommand {
    argument_hint: Option<String>,
    should_submit: bool,
}

/// Splice the active suggestion into the prompt according to `mode`.
pub fn accept_suggestion(state: &mut AppState, mode: AcceptMode) -> Option<CompletionInsertion> {
    if mode == AcceptMode::ExtendCommonPrefix && state.ui.input.accept_inline_ghost() {
        state.ui.completion.clear_active();
        state.ui.sync_popup_from_active_suggestions();
        return Some(CompletionInsertion {
            replacement: String::new(),
            cursor_position: state.ui.input.textarea.cursor(),
            keep_popup: false,
            should_submit: false,
        });
    }

    if mode == AcceptMode::SubmitSelected
        && state
            .ui
            .completion
            .active
            .as_ref()
            .is_some_and(|s| s.kind == SuggestionKind::Directory)
    {
        state.ui.completion.clear_active();
        state.ui.sync_popup_from_active_suggestions();
        return Some(CompletionInsertion {
            replacement: String::new(),
            cursor_position: state.ui.input.textarea.cursor(),
            keep_popup: false,
            should_submit: true,
        });
    }

    let sug = state.ui.completion.active.take()?;
    state.ui.completion.active_key = None;
    let Some(item) = sug.items.get(sug.selected).cloned() else {
        state.ui.completion.clear_active();
        state.ui.sync_popup_from_active_suggestions();
        return None;
    };

    let slash_command = if sug.kind == SuggestionKind::SlashCommand {
        accepted_slash_command(state, &item.label)
    } else {
        None
    };

    let insertion = if mode == AcceptMode::ExtendCommonPrefix {
        common_path_prefix_completion(&sug).unwrap_or_else(|| selected_insertion(&sug, &item, mode))
    } else {
        selected_insertion(&sug, &item, mode)
    };

    let text_len = state.ui.input.text().len();
    let start = sug.trigger_pos.min(text_len);
    let mut end = state.ui.input.textarea.cursor().min(text_len);
    if sug.query.starts_with('"')
        && state
            .ui
            .input
            .text()
            .get(end..)
            .is_some_and(|tail| tail.starts_with('"'))
    {
        end += 1;
    }
    state
        .ui
        .input
        .textarea
        .replace_range(start..end, &insertion.replacement);
    let cursor_position = start + insertion.cursor_position;
    state.ui.input.textarea.set_cursor(cursor_position);

    if let Some(hint) = slash_command
        .as_ref()
        .and_then(|cmd| cmd.argument_hint.as_ref())
    {
        state.ui.input.set_inline_hint(format!(" {hint}"));
    } else {
        state.ui.input.clear_inline_hint();
    }

    if insertion.keep_popup {
        crate::autocomplete::refresh_suggestions(state);
    } else {
        state.ui.sync_popup_from_active_suggestions();
    }

    let should_submit = insertion.should_submit
        || sug.kind == SuggestionKind::CustomTitle
        || slash_command.is_some_and(|cmd| cmd.should_submit);

    Some(CompletionInsertion {
        should_submit,
        ..insertion
    })
}

/// Disposition of an accepted *directory* row. Files always finalize, so this
/// only varies the directory case — see [`dir_accept_policy`].
#[derive(Debug, Clone, Copy)]
enum DirAccept {
    /// Finalize as a mention: append a trailing space so the popup closes (the
    /// trailing space leaves no live `@` token for re-detection to re-arm).
    Finalize,
    /// Drill in: append `/` with no trailing space, so the still-live token
    /// re-opens the popup on the directory's contents.
    Drill,
}

/// How an accepted directory splices into the prompt, mirroring TS
/// `useTypeahead`:
/// - **Enter** in the fuzzy `@` popup (`At`, TS `suggestionType: 'file'`)
///   finalizes the directory as a mention. Fuzzy search returns the directory
///   *itself* as a top hit, so re-searching the same token would loop — Enter
///   must commit, not drill.
/// - Everything else drills: **Tab** (common-prefix progress) and the
///   path-traversal kinds (`Path` / `Directory`, TS `suggestionType:
///   'directory'`), whose re-search lists the directory's *contents* and so
///   makes real progress instead of looping.
fn dir_accept_policy(kind: SuggestionKind, mode: AcceptMode) -> DirAccept {
    match (kind, mode) {
        (SuggestionKind::At, AcceptMode::AcceptSelected | AcceptMode::SubmitSelected) => {
            DirAccept::Finalize
        }
        _ => DirAccept::Drill,
    }
}

fn selected_insertion(
    sug: &ActiveSuggestions,
    item: &SuggestionItem,
    mode: AcceptMode,
) -> CompletionInsertion {
    let dir_accept = dir_accept_policy(sug.kind, mode);
    match sug.kind {
        SuggestionKind::SlashCommand => insertion(format!("{} ", item.label), false, false),
        SuggestionKind::Symbol => insertion(format!("@#{} ", item.label), false, false),
        SuggestionKind::Directory => format_path_insertion(item, "", &sug.query, dir_accept),
        SuggestionKind::At | SuggestionKind::Path => {
            format_at_insertion(item, &sug.query, dir_accept)
        }
        SuggestionKind::CustomTitle => insertion(item.label.clone(), false, false),
    }
}

fn insertion(replacement: String, keep_popup: bool, should_submit: bool) -> CompletionInsertion {
    CompletionInsertion {
        cursor_position: replacement.len(),
        replacement,
        keep_popup,
        should_submit,
    }
}

fn common_path_prefix_completion(sug: &ActiveSuggestions) -> Option<CompletionInsertion> {
    if !matches!(
        sug.kind,
        SuggestionKind::At | SuggestionKind::Path | SuggestionKind::Directory
    ) || sug.items.len() < 2
    {
        return None;
    }
    if !sug
        .items
        .iter()
        .all(|item| matches!(item.metadata.as_ref(), Some(SuggestionMeta::Path { .. })))
    {
        return None;
    }
    let prefix = common_prefix(sug.items.iter().map(|item| item.label.as_str()))?;
    if prefix.len() <= sug.query.trim_start_matches('"').len() {
        return None;
    }
    let token_prefix = if matches!(sug.kind, SuggestionKind::At | SuggestionKind::Path) {
        "@"
    } else {
        ""
    };
    Some(format_common_path_prefix_token(
        &prefix,
        token_prefix,
        &sug.query,
    ))
}

fn common_prefix<'a>(mut items: impl Iterator<Item = &'a str>) -> Option<String> {
    let first = items.next()?.to_string();
    let mut end = first.len();
    for item in items {
        end = common_prefix_len(&first[..end], item);
        if end == 0 {
            break;
        }
    }
    Some(first[..end].to_string())
}

fn common_prefix_len(a: &str, b: &str) -> usize {
    let mut end = 0;
    for ((a_idx, a_ch), (_, b_ch)) in a.char_indices().zip(b.char_indices()) {
        if a_ch != b_ch {
            break;
        }
        end = a_idx + a_ch.len_utf8();
    }
    end
}

fn accepted_slash_command(state: &AppState, label: &str) -> Option<AcceptedSlashCommand> {
    let name = label.strip_prefix('/')?;
    state
        .session
        .available_commands
        .iter()
        .find(|cmd| cmd.name == name)
        .map(|cmd| AcceptedSlashCommand {
            argument_hint: cmd.argument_hint.clone(),
            // Overlay-opening commands (`local-jsx` in TS — `/theme`, `/model`,
            // …) run on Enter even when they advertise an optional `[arg]`
            // hint: the overlay collects any further input interactively. Only
            // commands whose argument must be typed first stay in completion
            // mode after accepting the suggestion.
            should_submit: !cmd.argument_kind.accepts_arguments()
                || cmd.kind == CommandTypeTag::LocalOverlay,
        })
}

fn format_at_insertion(
    item: &SuggestionItem,
    query: &str,
    dir_accept: DirAccept,
) -> CompletionInsertion {
    match item.metadata.as_ref() {
        Some(SuggestionMeta::Path { .. }) => format_path_insertion(item, "@", query, dir_accept),
        Some(SuggestionMeta::Agent { .. }) => insertion(format!("@{} ", item.label), false, false),
        Some(SuggestionMeta::McpResource { server, uri }) => {
            insertion(format!("@{server}:{uri} "), false, false)
        }
        Some(SuggestionMeta::Symbol) => insertion(format!("@#{} ", item.label), false, false),
        Some(SuggestionMeta::Session) | None => {
            let body = quote_path_token(&item.label, false);
            insertion(format!("@{body} "), false, false)
        }
    }
}

fn format_path_insertion(
    item: &SuggestionItem,
    prefix: &str,
    query: &str,
    dir_accept: DirAccept,
) -> CompletionInsertion {
    match item.metadata.as_ref() {
        Some(SuggestionMeta::Path { is_directory }) => {
            format_path_token(&item.label, prefix, query, *is_directory, dir_accept)
        }
        _ => insertion(format!("{prefix}{} ", item.label), false, false),
    }
}

fn format_path_token(
    label: &str,
    prefix: &str,
    query: &str,
    is_directory: bool,
    dir_accept: DirAccept,
) -> CompletionInsertion {
    let mut path = label.to_string();
    if is_directory && !path.ends_with('/') {
        path.push('/');
    }
    let force_quote = query.starts_with('"');
    let quoted = force_quote || path_needs_quotes(&path);
    let body = quote_path_token(&path, quoted);

    // A file always finalizes (trailing space, popup closes); a directory's
    // disposition is set by `dir_accept` (trigger kind + keypress).
    let (append_space, keep_popup) = if is_directory {
        match dir_accept {
            DirAccept::Finalize => (true, false),
            DirAccept::Drill => (false, true),
        }
    } else {
        (true, false)
    };

    let mut replacement = format!("{prefix}{body}");
    if append_space {
        replacement.push(' ');
    }
    let cursor_position = if keep_popup && quoted {
        replacement.len().saturating_sub(1)
    } else {
        replacement.len()
    };
    CompletionInsertion {
        replacement,
        cursor_position,
        keep_popup,
        should_submit: false,
    }
}

fn format_common_path_prefix_token(
    prefix_value: &str,
    prefix: &str,
    query: &str,
) -> CompletionInsertion {
    let quoted = query.starts_with('"') || path_needs_quotes(prefix_value);
    let body = quote_path_token(prefix_value, quoted);
    let replacement = format!("{prefix}{body}");
    let cursor_position = if quoted {
        replacement.len().saturating_sub(1)
    } else {
        replacement.len()
    };
    CompletionInsertion {
        replacement,
        cursor_position,
        keep_popup: true,
        should_submit: false,
    }
}

fn path_needs_quotes(path: &str) -> bool {
    path.contains(char::is_whitespace) || path.contains('"') || path.contains('\\')
}

fn quote_path_token(path: &str, quoted: bool) -> String {
    if !quoted {
        return path.to_string();
    }
    let mut out = String::with_capacity(path.len() + 2);
    out.push('"');
    for ch in path.chars() {
        match ch {
            '"' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}
