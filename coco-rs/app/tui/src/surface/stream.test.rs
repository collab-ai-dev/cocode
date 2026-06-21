use super::*;
use crate::state::AppState;
use crate::state::PanePromptState;
use crate::state::PermissionDetail;
use crate::state::PermissionPromptState;
use crate::state::ui::StreamingState;
use crate::surface::modal::HistorySurfaceMode;

fn native_plan() -> SurfaceFramePlan {
    SurfaceFramePlan {
        modal_placement: None,
        history_surface: HistorySurfaceMode::NativeScrollback,
        attention_requested: false,
    }
}

/// A long bullet list with NO trailing blank line stays entirely in the mutable
/// tail (the markdown stable boundary only commits at blank lines / closed
/// fences), and each item renders to its own row — so the tail (one Line per
/// item) exceeds the cap. Lists/code blocks are what actually grow the viewport
/// row count (soft-wrapped prose is one logical Line wrapped at paint).
fn streaming_state_long_tail() -> AppState {
    let mut state = AppState::new();
    let mut streaming = StreamingState::new();
    let list: String = (0..15).map(|i| format!("- item number {i}\n")).collect();
    streaming.append_text(&list);
    streaming.reveal_all();
    state.ui.streaming = Some(streaming);
    state
}

#[test]
fn prepare_caps_streaming_tail_to_constant_height() {
    let state = streaming_state_long_tail();
    let mut driver = SurfaceStreamDriver::default();
    let prepared = driver.prepare(&state, /*width*/ 24, native_plan());
    assert!(
        prepared.lines.len() <= STREAMING_LIVE_TAIL_CAP as usize,
        "streaming tail must be capped to {} rows, got {}",
        STREAMING_LIVE_TAIL_CAP,
        prepared.lines.len()
    );
}

#[test]
fn cap_is_display_only_and_uncapped_scroll_shows_full_stream() {
    let build = |user_scrolled: bool| {
        let mut state = AppState::new();
        let mut streaming = StreamingState::new();
        let mut src = String::from("committed paragraph\n\n");
        src.push_str(&(0..15).map(|i| format!("- item {i}\n")).collect::<String>());
        streaming.append_text(&src);
        streaming.reveal_all();
        state.ui.streaming = Some(streaming);
        state.ui.user_scrolled = user_scrolled;
        SurfaceStreamDriver::default().prepare(&state, /*width*/ 40, native_plan())
    };
    let capped = build(/*user_scrolled*/ false);
    let uncapped = build(/*user_scrolled*/ true);

    assert!(capped.lines.len() <= STREAMING_LIVE_TAIL_CAP as usize);
    assert!(uncapped.lines.len() > STREAMING_LIVE_TAIL_CAP as usize);
    let append_text = history_rows_text(
        &uncapped
            .stream_append
            .as_ref()
            .expect("stable prefix append")
            .rows,
    );
    assert!(append_text.contains("committed paragraph"));
    let uncapped_text = plain_lines(&uncapped.lines).join("\n");
    assert!(uncapped_text.contains("item 14"));
}

#[test]
fn stable_stream_prefix_prepares_native_append_and_leaves_viewport_tail() {
    let mut state = AppState::new();
    let mut streaming = StreamingState::new();
    streaming.append_text("alpha\n\n");
    streaming.reveal_all();
    state.ui.streaming = Some(streaming);

    let mut driver = SurfaceStreamDriver::default();
    let first = driver.prepare(&state, /*width*/ 40, native_plan());
    let first_append = first.stream_append.as_ref().expect("stream append");
    assert!(history_rows_text(&first_append.rows).contains("alpha"));
    assert!(!plain_lines(&first.lines).join("\n").contains("alpha"));
    driver.mark_stream_append_committed(first_append);

    let streaming = state.ui.streaming.as_mut().expect("streaming");
    streaming.append_text("beta\n\n");
    streaming.reveal_all();
    let second = driver.prepare(&state, /*width*/ 40, native_plan());
    let second_append = second.stream_append.as_ref().expect("stream append");
    let second_append_text = history_rows_text(&second_append.rows);
    let second_text = plain_lines(&second.lines).join("\n");

    assert_eq!(
        second_append_text.matches("alpha").count(),
        0,
        "{second_append_text}"
    );
    assert_eq!(
        second_append_text.matches("beta").count(),
        1,
        "{second_append_text}"
    );
    assert_eq!(second_text.matches("alpha").count(), 0, "{second_text}");
    assert_eq!(second_text.matches("beta").count(), 0, "{second_text}");
}

#[test]
fn watermark_does_not_survive_source_replacement() {
    // Event coalescing can fold `MessageAppended(turn N)` and turn N+1's first
    // deltas into one draw, so this driver never observes the `streaming ==
    // None` gap that normally clears the watermark. Turn N+1's source replaces
    // turn N's (no `starts_with` extension), and if its first observed stable
    // region is already larger than turn N's watermark in both bytes and
    // lines, a length-only validity check would re-attribute the old watermark
    // to the new turn and silently skip its leading rows. The replacement must
    // invalidate instead: full stable region re-emitted + replay signalled.
    let mut state = AppState::new();
    let mut streaming = StreamingState::new();
    streaming.append_text("done\n\n");
    streaming.reveal_all();
    state.ui.streaming = Some(streaming);
    let mut driver = SurfaceStreamDriver::default();
    let first = driver.prepare(&state, /*width*/ 40, native_plan());
    driver.mark_stream_append_committed(first.stream_append.as_ref().expect("turn-1 append"));

    let mut streaming = StreamingState::new();
    streaming.append_text("alpha paragraph one\n\nbeta paragraph two\n\ngamma three\n\n");
    streaming.reveal_all();
    state.ui.streaming = Some(streaming);
    let second = driver.prepare(&state, /*width*/ 40, native_plan());

    assert!(
        second.commit_invalidated,
        "source replacement must invalidate the surviving commit",
    );
    let append = second.stream_append.expect("turn-2 stable append");
    let text = history_rows_text(&append.rows);
    assert!(
        text.contains("alpha paragraph one"),
        "turn-2's leading rows must not be skipped: {text}",
    );
}

#[test]
fn transient_streaming_gap_does_not_re_emit_committed_rows() {
    // The duplication root cause (tui-v2 §6.7-10): event coalescing can fold a
    // turn's last delta and the next's first into one draw, so a `streaming ==
    // None` frame is NOT a reliable end signal. The OLD code cleared the
    // watermark on that gap, so the next active frame re-committed the rows
    // ALREADY in scrollback — the leading text appeared twice. The single
    // commit must survive the gap so only the increment is appended.
    let mut state = AppState::new();
    let mut streaming = StreamingState::new();
    streaming.append_text("alpha\n\n");
    streaming.reveal_all();
    state.ui.streaming = Some(streaming);

    let mut driver = SurfaceStreamDriver::default();
    let first = driver.prepare(&state, /*width*/ 40, native_plan());
    driver.mark_stream_append_committed(first.stream_append.as_ref().expect("turn-1 append"));

    // Transient gap: streaming momentarily None, no finalize.
    state.ui.streaming = None;
    let gap = driver.prepare(&state, /*width*/ 40, native_plan());
    assert!(gap.stream_append.is_none());
    assert!(
        !gap.commit_invalidated,
        "a transient gap must not invalidate"
    );

    // Resume the SAME message with more content.
    let mut streaming = StreamingState::new();
    streaming.append_text("alpha\n\nbeta\n\n");
    streaming.reveal_all();
    state.ui.streaming = Some(streaming);
    let resumed = driver.prepare(&state, /*width*/ 40, native_plan());

    assert!(
        !resumed.commit_invalidated,
        "resuming the same document must not invalidate the commit",
    );
    let append = resumed
        .stream_append
        .as_ref()
        .expect("resumed stable append");
    let text = history_rows_text(&append.rows);
    assert_eq!(
        text.matches("alpha").count(),
        0,
        "already-committed leading rows must NOT be re-emitted: {text}",
    );
    assert_eq!(
        text.matches("beta").count(),
        1,
        "only the increment: {text}"
    );
}

#[test]
fn no_stream_frame_projects_pending_exit_plan_without_stream_append() {
    let state = exit_plan_prompt_state(12);
    let mut driver = SurfaceStreamDriver::default();

    let prepared = driver.prepare(&state, /*width*/ 64, native_plan());
    let text = plain_lines(&prepared.lines).join("\n");

    assert!(prepared.stream_append.is_none());
    assert!(!prepared.commit_invalidated);
    assert!(text.contains("Here is proposed plan:"), "{text}");
    assert!(text.contains("step 12"), "{text}");
    assert!(text.contains("Plan file: /tmp/session-plan.md"), "{text}");
    assert!(!text.contains("manually approve edits"), "{text}");
}

#[test]
fn pending_exit_plan_disappears_after_prompt_closes() {
    let mut state = exit_plan_prompt_state(3);
    let mut driver = SurfaceStreamDriver::default();

    let pending = driver.prepare(&state, /*width*/ 64, native_plan());
    assert!(
        plain_lines(&pending.lines)
            .join("\n")
            .contains("Here is proposed plan:")
    );
    assert!(
        driver.pending_plan.is_some(),
        "pending projection should be cached while prompt is active"
    );

    state.ui.interaction.active_prompt = None;
    let closed = driver.prepare(&state, /*width*/ 64, native_plan());

    assert!(closed.lines.is_empty(), "{:?}", plain_lines(&closed.lines));
    assert!(closed.stream_append.is_none());
    assert!(
        driver.pending_plan.is_none(),
        "pending projection cache must clear when prompt closes"
    );
}

#[test]
fn invalidated_commit_re_emits_full_stable_prefix() {
    // The loss root cause, opposite sign: when a replay clears owned scrollback
    // the controller calls `invalidate_commit`, after which the next prepare
    // must re-emit the FULL stable prefix (the rows the clear just wiped), not
    // only the increment. The OLD split watermark survived the replay and
    // dropped the leading rows permanently.
    let mut state = AppState::new();
    let mut streaming = StreamingState::new();
    streaming.append_text("alpha\n\n");
    streaming.reveal_all();
    state.ui.streaming = Some(streaming);

    let mut driver = SurfaceStreamDriver::default();
    let first = driver.prepare(&state, /*width*/ 40, native_plan());
    driver.mark_stream_append_committed(first.stream_append.as_ref().expect("turn-1 append"));

    // A replay cleared scrollback: the controller invalidates the commit.
    driver.invalidate_commit();

    let mut streaming = StreamingState::new();
    streaming.append_text("alpha\n\nbeta\n\n");
    streaming.reveal_all();
    state.ui.streaming = Some(streaming);
    let after = driver.prepare(&state, /*width*/ 40, native_plan());
    let append = after.stream_append.as_ref().expect("re-emitted append");
    let text = history_rows_text(&append.rows);
    assert_eq!(
        text.matches("alpha").count(),
        1,
        "the wiped leading rows must be re-emitted in full: {text}",
    );
    assert_eq!(text.matches("beta").count(), 1, "{text}");
}

#[test]
fn prepare_does_not_cap_while_user_is_scrolling() {
    let mut state = streaming_state_long_tail();
    state.ui.user_scrolled = true;
    let mut driver = SurfaceStreamDriver::default();
    let prepared = driver.prepare(&state, /*width*/ 24, native_plan());
    // The same content un-capped renders to MORE than the cap — proving the
    // input genuinely overflows and that the cap (not a short render) is what
    // bounds the other test, and that an actively-scrolling user sees it all.
    assert!(
        prepared.lines.len() > STREAMING_LIVE_TAIL_CAP as usize,
        "while scrolling the full tail is shown; got {} rows (cap {})",
        prepared.lines.len(),
        STREAMING_LIVE_TAIL_CAP
    );
}

fn plain_lines(lines: &[ratatui::text::Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

fn history_rows_text(rows: &coco_tui_ui::engine::history_insert::HistoryRows) -> String {
    rows.buffer()
        .content
        .chunks(rows.width() as usize)
        .map(|cells| {
            cells
                .iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn exit_plan_prompt_state(step_count: usize) -> AppState {
    let mut state = AppState::new();
    let plan: String = (1..=step_count).map(|i| format!("- step {i}\n")).collect();
    state
        .ui
        .push_prompt(PanePromptState::Permission(PermissionPromptState {
            request_id: "req-1".into(),
            tool_name: coco_types::ToolName::ExitPlanMode.as_str().into(),
            description: "Exit plan mode?".into(),
            detail: PermissionDetail::ExitPlanMode {
                outcome: coco_types::ExitPlanModeOutcome::ImplementationPlan,
                plan: Some(plan),
                plan_file_path: Some("/tmp/session-plan.md".into()),
                allowed_prompts: vec![],
            },
            risk_level: None,
            show_always_allow: false,
            classifier_checking: false,
            classifier_auto_approved: None,
            choices: Some(vec![
                coco_types::PermissionAskChoice {
                    value: coco_types::ExitPlanChoice::ClearAcceptEdits.as_str().into(),
                    label: "Yes, clear context and auto-accept edits".into(),
                    description: None,
                },
                coco_types::PermissionAskChoice {
                    value: coco_types::ExitPlanChoice::KeepDefault.as_str().into(),
                    label: "Yes, manually approve edits".into(),
                    description: None,
                },
                coco_types::PermissionAskChoice {
                    value: coco_types::ExitPlanChoice::No.as_str().into(),
                    label: "No, keep planning".into(),
                    description: None,
                },
            ]),
            selected_choice: 0,
            display_input: coco_types::PermissionDisplayInput::Empty,
            original_input: None,
            cwd: None,
            permission_suggestions: vec![],
            worker_badge: None,
            explanation_visible: false,
            explanation: crate::state::ExplainerFetch::NotFetched,
            prefix_input: None,
        }));
    state
}
