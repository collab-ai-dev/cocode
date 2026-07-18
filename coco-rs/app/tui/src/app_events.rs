use super::*;
pub(super) fn memory_perf_interval(
    config: crate::display_settings::TuiPerformanceConfig,
) -> tokio::time::Interval {
    let duration = crate::perf::MemoryPerfTracker::periodic_interval(config);
    let mut interval = interval_at(tokio::time::Instant::now() + duration, duration);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    interval
}

/// Resolve a pending hint against the on-disk marketplace cache. Builds a
/// short-lived [`coco_plugins::marketplace::MarketplaceManager`] rooted at
/// the user plugins dir, loads the hint's marketplace from disk, and looks
/// up the plugin entry. Returns `None` if the marketplace or plugin isn't cached.
pub(super) fn resolve_pending_plugin_hint(
    hint: &coco_plugins::ClaudeCodeHint,
) -> Option<coco_plugins::PluginRecommendation> {
    let (_, marketplace_name) = hint.value.split_once('@')?;
    let plugins_dir = coco_config::global_config::config_home().join("plugins");
    let mut manager = coco_plugins::marketplace::MarketplaceManager::new(plugins_dir);
    // Load the marketplace into the in-memory cache from its disk location.
    // Missing / unparseable cache → the hint is discarded.
    manager.load_cached_marketplace(marketplace_name).ok()?;
    coco_plugins::resolve_plugin_hint(hint, &manager)
}

pub(super) fn apply_terminal_compatibility_status<B>(state: &mut AppState, tui: &Tui<B>)
where
    B: SurfaceBackend,
{
    if let Some(message) = tui.native_scrollback_status_message() {
        let message = message.to_string();
        state.ui.terminal_compatibility_warning = Some(message.clone());
        state.ui.add_toast(Toast::warning(message));
    }
}

/// Apply a file-search result to `active_suggestions`.
///
/// Drops the result when the user has moved on to a different query,
/// different trigger kind, or dismissed the popup altogether. That
/// guarantees a slow search started when the user typed `@src` doesn't
/// clobber the state after they've backspaced past the trigger.
pub(super) fn handle_file_search_event(state: &mut AppState, evt: FileSearchEvent) -> bool {
    match evt {
        FileSearchEvent::SearchResult { key, suggestions } => {
            apply_async_result_for_key(state, &key, suggestions)
        }
    }
}

pub(super) fn handle_path_completion_event(state: &mut AppState, evt: PathCompletionEvent) -> bool {
    match evt {
        PathCompletionEvent::SearchResult { key, suggestions } => {
            apply_async_result_for_key(state, &key, suggestions)
        }
    }
}

pub(super) fn handle_symbol_search_event(state: &mut AppState, evt: SymbolSearchEvent) -> bool {
    match evt {
        SymbolSearchEvent::SearchResult { key, suggestions } => {
            apply_async_result_for_key(state, &key, suggestions)
        }
    }
}

use crate::autocomplete::apply_async_result_for_key;

/// Helper: receive from an `Option<Receiver<T>>`. Returns `None`
/// (the receiver-closed case) when the option itself is None — the
/// `if self.kb_warnings_rx.is_some()` guard in `tokio::select!`
/// already ensures we never enter the `match` arm without a channel.
pub(super) async fn recv_optional<T>(rx: &mut Option<mpsc::Receiver<T>>) -> Option<T> {
    match rx.as_mut() {
        Some(r) => r.recv().await,
        None => std::future::pending().await,
    }
}

/// Map a voice-capture/start failure to a concise, user-facing footer message
/// (the typed error strings from the design).
pub(super) fn voice_error_text(err: &coco_voice::VoiceError) -> String {
    use coco_voice::VoiceError;
    match err {
        VoiceError::NoAudioDevice => "No microphone detected.".to_string(),
        VoiceError::NoSpeechDetected => "No speech detected.".to_string(),
        VoiceError::FeatureNotEnabled(backend) => {
            format!("Voice backend `{backend}` is not available in this build.")
        }
        VoiceError::Connection(_) => "Voice connection failed. Check your network.".to_string(),
        other => format!("Voice error: {other}"),
    }
}

/// Push every keybinding warning as its own toast (error vs warning
/// styling). Empty input is a no-op (returned by hot-reload paths
/// where the new config is clean). Returns `true` if at least one
/// toast was added so the caller redraws.
pub(super) fn surface_keybinding_warnings(
    state: &mut AppState,
    issues: Vec<coco_keybindings::ValidationIssue>,
) -> bool {
    if issues.is_empty() {
        return false;
    }
    for issue in issues {
        let line = coco_keybindings::format_issue_oneline(&issue);
        let toast = match issue.severity {
            coco_keybindings::Severity::Error => crate::state::ui::Toast::error(line),
            coco_keybindings::Severity::Warning => crate::state::ui::Toast::warning(line),
        };
        state.ui.add_toast(toast);
    }
    true
}

pub(super) fn apply_theme_reload(
    state: &mut AppState,
    result: crate::theme::ThemeLoadResult,
) -> bool {
    if let Some(error) = result.error {
        state.ui.add_toast(crate::state::ui::Toast::warning(error));
        return true;
    }
    state.ui.apply_theme_runtime(result.state);
    true
}

pub(super) fn is_external_editor_completion(event: &CoreEvent) -> bool {
    matches!(
        event,
        CoreEvent::Tui(
            TuiOnlyEvent::MemoryFileOpened { .. }
                | TuiOnlyEvent::MemoryFileOpenFailed { .. }
                | TuiOnlyEvent::PlanFileOpened { .. }
                | TuiOnlyEvent::PlanFileOpenFailed { .. }
                | TuiOnlyEvent::ExitPlanPromptEditorCompleted { .. }
                | TuiOnlyEvent::ExitPlanPromptEditorFailed { .. }
                | TuiOnlyEvent::PromptEditorCompleted { .. }
                | TuiOnlyEvent::PromptEditorFailed { .. }
        )
    )
}

pub(super) enum DeferredCoreEvent {
    Buffered,
    Dropped,
    ProcessNow(Box<CoreEvent>),
}

pub(super) fn defer_core_event(
    buffer: &mut VecDeque<CoreEvent>,
    event: CoreEvent,
) -> DeferredCoreEvent {
    if coalesce_lossy_deferred_event(buffer, &event) {
        return DeferredCoreEvent::Buffered;
    }
    if buffer.len() < DEFERRED_CORE_EVENT_LIMIT {
        buffer.push_back(event);
        return DeferredCoreEvent::Buffered;
    }
    if is_lossy_deferred_event(&event) {
        return DeferredCoreEvent::Dropped;
    }
    if let Some(pos) = buffer.iter().position(is_lossy_deferred_event) {
        buffer.remove(pos);
        buffer.push_back(event);
        return DeferredCoreEvent::Buffered;
    }
    let Some(oldest) = buffer.pop_front() else {
        buffer.push_back(event);
        return DeferredCoreEvent::Buffered;
    };
    buffer.push_back(event);
    DeferredCoreEvent::ProcessNow(Box::new(oldest))
}

pub(super) fn coalesce_lossy_deferred_event(
    buffer: &mut VecDeque<CoreEvent>,
    event: &CoreEvent,
) -> bool {
    match event {
        CoreEvent::Stream(AgentStreamEvent::TextDelta { turn_id, delta }) => {
            for existing in buffer.iter_mut().rev() {
                if let CoreEvent::Stream(AgentStreamEvent::TextDelta {
                    turn_id: existing_turn_id,
                    delta: existing_delta,
                }) = existing
                    && existing_turn_id == turn_id
                {
                    existing_delta.push_str(delta);
                    return true;
                }
            }
            false
        }
        CoreEvent::Stream(AgentStreamEvent::ThinkingDelta { turn_id, delta }) => {
            for existing in buffer.iter_mut().rev() {
                if let CoreEvent::Stream(AgentStreamEvent::ThinkingDelta {
                    turn_id: existing_turn_id,
                    delta: existing_delta,
                }) = existing
                    && existing_turn_id == turn_id
                {
                    existing_delta.push_str(delta);
                    return true;
                }
            }
            false
        }
        CoreEvent::Tui(TuiOnlyEvent::ToolCallDelta { call_id, delta }) => {
            for existing in buffer.iter_mut().rev() {
                if let CoreEvent::Tui(TuiOnlyEvent::ToolCallDelta {
                    call_id: existing_call_id,
                    delta: existing_delta,
                }) = existing
                    && existing_call_id == call_id
                {
                    existing_delta.push_str(delta);
                    return true;
                }
            }
            false
        }
        CoreEvent::Tui(TuiOnlyEvent::ToolProgress { tool_use_id, .. }) => {
            replace_matching_deferred(buffer, event, |candidate| {
                matches!(
                    candidate,
                    CoreEvent::Tui(TuiOnlyEvent::ToolProgress {
                        tool_use_id: existing_tool_use_id,
                        ..
                    }) if existing_tool_use_id == tool_use_id
                )
            })
        }
        CoreEvent::Protocol(ServerNotification::TaskProgress(p)) => {
            for existing_event in buffer.iter_mut().rev() {
                if let CoreEvent::Protocol(ServerNotification::TaskProgress(existing)) =
                    existing_event
                    && existing.task_id == p.task_id
                {
                    let mut merged = p.clone();
                    merged.workflow_progress = merge_deferred_workflow_progress(
                        &existing.workflow_progress,
                        &p.workflow_progress,
                    );
                    *existing = merged;
                    return true;
                }
            }
            false
        }
        CoreEvent::Protocol(ServerNotification::ToolProgress(p)) => {
            replace_matching_deferred(buffer, event, |candidate| {
                matches!(
                    candidate,
                    CoreEvent::Protocol(ServerNotification::ToolProgress(existing))
                        if existing.tool_use_id == p.tool_use_id
                )
            })
        }
        _ => false,
    }
}

pub(super) fn handle_classifier_denied(
    state: &mut AppState,
    request_id: &str,
    reason: &str,
) -> bool {
    let Some(crate::state::PanePromptState::Permission(prompt)) =
        state.ui.interaction.active_prompt.as_mut()
    else {
        return false;
    };
    if prompt.request_id != request_id {
        return false;
    }

    prompt.classifier_checking = false;
    let tool_name = prompt.tool_name.clone();
    let display = prompt.description.clone();
    let message = auto_mode_denied_toast_message(&tool_name, reason);
    state
        .ui
        .record_recent_denial(tool_name, display, reason.to_string());
    state.ui.add_toast(Toast::warning(message));
    true
}

pub(crate) fn auto_mode_denied_toast_message(tool_name: &str, reason: &str) -> String {
    let reason = coco_utils_string::truncate_utf16_units_with_ellipsis(reason, 80, 79, "…");
    let tool = tool_name.to_lowercase();
    if reason.is_empty() {
        format!("{tool} denied by auto mode · /permissions")
    } else {
        format!("{tool} denied by auto mode · {reason} · /permissions")
    }
}

pub(super) fn convert_crossterm_event(event: Event) -> Option<TuiEvent> {
    match event {
        Event::Key(key) => {
            if !should_accept_key_event(&key) {
                return None;
            }
            // Intercept Ctrl+Z before keybinding dispatch so the
            // user can never accidentally remap process suspend.
            // Raw mode would otherwise eat the keystroke silently.
            // On non-Unix it falls through as a normal Key event
            // (no SIGTSTP semantics anyway).
            #[cfg(unix)]
            if key.code == KeyCode::Char('z') && key.modifiers == KeyModifiers::CONTROL {
                return Some(TuiEvent::Suspend);
            }
            Some(TuiEvent::Key(key))
        }
        // We never call EnableMouseCapture, so crossterm shouldn't deliver
        // Event::Mouse in practice — drop it defensively if it ever arrives.
        Event::Mouse(_) => None,
        Event::Resize(w, h) => Some(TuiEvent::Resize {
            width: w,
            height: h,
        }),
        Event::FocusGained => Some(TuiEvent::FocusChanged { focused: true }),
        Event::FocusLost => Some(TuiEvent::FocusChanged { focused: false }),
        Event::Paste(text) => Some(TuiEvent::Paste(text)),
    }
}

pub(super) fn should_accept_key_event(key: &KeyEvent) -> bool {
    match key.kind {
        KeyEventKind::Press => true,
        KeyEventKind::Repeat => is_repeat_safe_key(key),
        KeyEventKind::Release => false,
    }
}

pub(super) fn is_repeat_safe_key(key: &KeyEvent) -> bool {
    match key.code {
        KeyCode::Left
        | KeyCode::Right
        | KeyCode::Up
        | KeyCode::Down
        | KeyCode::Home
        | KeyCode::End
        | KeyCode::PageUp
        | KeyCode::PageDown
        | KeyCode::Backspace
        | KeyCode::Delete => true,
        KeyCode::Char(_) => !key.modifiers.intersects(
            KeyModifiers::CONTROL
                | KeyModifiers::ALT
                | KeyModifiers::SUPER
                | KeyModifiers::HYPER
                | KeyModifiers::META,
        ),
        KeyCode::BackTab
        | KeyCode::Enter
        | KeyCode::Esc
        | KeyCode::F(_)
        | KeyCode::Insert
        | KeyCode::CapsLock
        | KeyCode::ScrollLock
        | KeyCode::NumLock
        | KeyCode::PrintScreen
        | KeyCode::Pause
        | KeyCode::Menu
        | KeyCode::KeypadBegin
        | KeyCode::Media(_)
        | KeyCode::Modifier(_)
        | KeyCode::Null
        | KeyCode::Tab => false,
    }
}

pub(super) fn merge_deferred_workflow_progress(
    existing: &[coco_types::WorkflowProgressEvent],
    incoming: &[coco_types::WorkflowProgressEvent],
) -> Vec<coco_types::WorkflowProgressEvent> {
    if incoming.is_empty() {
        return existing.to_vec();
    }
    if incoming.starts_with(existing) {
        return incoming.to_vec();
    }
    let mut merged = existing.to_vec();
    merged.extend_from_slice(incoming);
    merged
}

pub(super) fn replace_matching_deferred(
    buffer: &mut VecDeque<CoreEvent>,
    event: &CoreEvent,
    matches_event: impl Fn(&CoreEvent) -> bool,
) -> bool {
    if let Some(existing) = buffer
        .iter_mut()
        .rev()
        .find(|candidate| matches_event(candidate))
    {
        *existing = event.clone();
        true
    } else {
        false
    }
}

pub(super) fn is_lossy_deferred_event(event: &CoreEvent) -> bool {
    matches!(
        event,
        CoreEvent::Stream(
            AgentStreamEvent::TextDelta { .. } | AgentStreamEvent::ThinkingDelta { .. }
        ) | CoreEvent::Tui(TuiOnlyEvent::ToolCallDelta { .. } | TuiOnlyEvent::ToolProgress { .. })
            | CoreEvent::Protocol(
                ServerNotification::AgentMessageDelta(_)
                    | ServerNotification::ReasoningDelta(_)
                    | ServerNotification::TaskProgress(_)
                    | ServerNotification::ToolProgress(_)
            )
    )
}
