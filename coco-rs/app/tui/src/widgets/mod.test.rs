//! Snapshot tests for TUI widgets using insta + native surface test rendering.

use crate::i18n::locale_test_guard;
use crate::state::AppState;
use crate::state::PermissionDetail;
use crate::state::PermissionPromptState;
use crate::state::StreamingState;
use crate::state::SuggestionKind;
use crate::state::Toast;
use crate::transcript::derive::test_helpers;

fn mark_retained_surface_visible(state: &mut AppState) {
    state
        .ui
        .record_surface_interaction(std::time::Instant::now());
}

fn render_to_string(state: &AppState, width: u16, height: u16) -> String {
    let _locale = locale_test_guard("en");
    crate::testing::render_native_surface_to_string(state, width, height)
}

#[test]
fn test_snapshot_empty_state() {
    let state = AppState::new();
    let output = render_to_string(&state, 80, 24);
    insta::assert_snapshot!("empty_state", output);
}

#[test]
fn test_snapshot_status_bar_full() {
    use std::sync::Arc;

    use coco_messages::AssistantContent;
    use coco_messages::TextContent;
    use coco_messages::create_assistant_message;
    use coco_types::ModelRole;

    use crate::state::ModelBinding;
    use crate::state::session::TokenUsage;

    let mut state = AppState::new();
    state.session.model = "deepseek-v4-flash".to_string();
    state.session.provider = "deepseek".to_string();
    state.session.token_usage = TokenUsage {
        input_tokens: 12_300,
        output_tokens: 16_500,
        reasoning_tokens: 0,
        cache_read_tokens: 10_700,
        cache_creation_tokens: 0,
    };
    state.session.model_by_role.insert(
        ModelRole::Main,
        ModelBinding {
            provider: "deepseek".to_string(),
            model_id: "deepseek-v4-flash".to_string(),
            context_window: Some(200_000),
            effort: None,
        },
    );
    state
        .session
        .transcript
        .on_message_appended(Arc::new(create_assistant_message(
            vec![AssistantContent::Text(TextContent {
                text: "ready".into(),
                provider_metadata: None,
            })],
            "deepseek-v4-flash",
            coco_types::TokenUsage {
                input_tokens: coco_types::InputTokens {
                    total: 20_000,
                    ..Default::default()
                },
                output_tokens: coco_types::OutputTokens {
                    total: 8_000,
                    ..Default::default()
                },
            },
        )));
    let output = render_to_string(&state, 100, 24);
    insta::assert_snapshot!("status_bar_full", output);
}

#[test]
fn test_snapshot_with_messages() {
    let mut state = AppState::new();
    state.session.model = "opus-4".to_string();
    test_helpers::push_user_text(&mut state.session, "1", "Hello, how are you?");
    test_helpers::push_assistant_text(
        &mut state.session,
        "I'm doing well! How can I help you today?",
    );

    let output = render_to_string(&state, 80, 24);
    insta::assert_snapshot!("with_messages", output);
}

#[test]
fn test_snapshot_thinking_collapsed() {
    let mut state = AppState::new();
    state.session.model = "opus-4".to_string();
    test_helpers::push_user_text(&mut state.session, "1", "hello");
    test_helpers::push_assistant_thinking(
        &mut state.session,
        "The user just said hello, so respond briefly.",
        1600,
        22,
    );
    test_helpers::push_assistant_text(&mut state.session, "Hello! How can I help?");

    let output = render_to_string(&state, 96, 18);
    insta::assert_snapshot!("thinking_collapsed", output);
}

#[test]
fn test_snapshot_thinking_expanded_show_thinking_on() {
    let mut state = AppState::new();
    state.session.model = "opus-4".to_string();
    state.ui.show_thinking = true;
    test_helpers::push_user_text(&mut state.session, "1", "bash ls");
    test_helpers::push_assistant_thinking(
        &mut state.session,
        "The user wants me to run `ls` in the current working directory.\n\
         I should call the Bash tool and then summarize the result.",
        1300,
        15,
    );
    test_helpers::push_tool_use(&mut state.session, "call-1", "Bash", "ls");
    test_helpers::push_tool_result(
        &mut state.session,
        "call-1",
        "Bash",
        "app\nbridge\ncommon\nutils\nvercel-ai",
        false,
    );

    let output = render_to_string(&state, 96, 22);
    insta::assert_snapshot!("thinking_expanded_show_thinking_on", output);
}

#[test]
fn test_snapshot_with_tool_result() {
    let mut state = AppState::new();
    state.session.model = "opus-4".to_string();
    test_helpers::push_user_text(&mut state.session, "1", "List files");
    // Object input (production shape) so the header reads `Bash(ls -la)`.
    test_helpers::push_tool_use_input(
        &mut state.session,
        "c1",
        "Bash",
        serde_json::json!({"command": "ls -la"}),
    );
    test_helpers::push_tool_result(
        &mut state.session,
        "c1",
        "Bash",
        "file1.rs\nfile2.rs\nCargo.toml",
        false,
    );

    let output = render_to_string(&state, 80, 24);
    insta::assert_snapshot!("with_tool_result", output);
}

#[test]
fn test_snapshot_subagent_result_summary() {
    // A finished subagent drops out of the transient panel; its run summary
    // surfaces on the committed Agent tool-result cell instead
    // (`└ ✓ Explore · 37 tools · 1m11s · ↑68.1k ↓468 · cache 95% · $0.18`).
    use crate::state::session::SubagentRunSummary;
    let mut state = AppState::new();
    state.session.model = "opus-4".to_string();
    test_helpers::push_user_text(&mut state.session, "1", "explore the codebase");
    test_helpers::push_tool_use_input(
        &mut state.session,
        "call-agent",
        "Agent",
        serde_json::json!({"description": "Map app/core crates", "subagent_type": "Explore"}),
    );
    test_helpers::push_tool_result(
        &mut state.session,
        "call-agent",
        "Agent",
        "Mapped the app/core crates and their responsibilities.",
        false,
    );
    state.session.insert_subagent_summary(
        "call-agent".into(),
        SubagentRunSummary {
            agent_type: "Explore".into(),
            tool_count: 37,
            duration_ms: 71_000,
            input_tokens: 68_100,
            output_tokens: 468,
            cache_read_tokens: 64_695,
            cost_usd: 0.18,
            succeeded: true,
        },
    );

    let output = render_to_string(&state, 100, 20);
    insta::assert_snapshot!("subagent_result_summary", output);
}

#[test]
fn test_snapshot_attachment_chips() {
    // Alignment reference: every leading marker is width-1 and content lands at
    // column 2 — user `❯`, tool `●`, result `└` (+ line-number gutter), memory
    // `◆` (filled, relative path), generic attachment `◇` (hollow), assistant
    // `⏺`. Locks the gutter, the `· lines N-M` header, and path relativization.
    use std::sync::Arc;

    use coco_messages::AttachmentMessage;
    use coco_messages::LlmMessage;
    use coco_messages::Message;
    use coco_types::AttachmentKind;

    let mut state = AppState::new();
    state.session.model = "opus-4".to_string();
    state.session.working_dir = Some("/repo".to_string());
    test_helpers::push_user_text(&mut state.session, "1", "read utils/foo/bar.rs");
    test_helpers::push_tool_use_input(
        &mut state.session,
        "c1",
        "Read",
        serde_json::json!({"file_path": "utils/foo/bar.rs", "limit": 3}),
    );
    test_helpers::push_tool_result(
        &mut state.session,
        "c1",
        "Read",
        "1\tfn foo() {}\n2\t\n3\t// done",
        false,
    );
    // Nested-CLAUDE.md injection → `◆ memory · <path relative to cwd>`.
    state
        .session
        .transcript
        .on_message_appended(Arc::new(Message::Attachment(AttachmentMessage::api(
            AttachmentKind::NestedMemory,
            LlmMessage::user_text(
                "<system-reminder>\nContents of /repo/utils/foo/CLAUDE.md:\n\n# foo rules\n</system-reminder>",
            ),
        ))));
    // Resolved @-mention summary → compact `└ Read <path> (N lines)` row.
    // (The raw `@-mentioned files` system-reminder is suppressed; the display
    // rides the typed `MentionSummary` extras.)
    state
        .session
        .transcript
        .on_message_appended(Arc::new(Message::Attachment(
            AttachmentMessage::mention_summary(coco_messages::MentionSummaryPayload {
                items: vec![coco_messages::MentionSummaryItem {
                    display_path: "utils/foo/bar.rs".to_string(),
                    kind: coco_messages::MentionItemKind::File,
                    count: Some(3),
                    truncated: false,
                }],
            }),
        )));
    test_helpers::push_assistant_text(&mut state.session, "Here is the file.");

    let output = render_to_string(&state, 80, 24);
    insta::assert_snapshot!("attachment_chips", output);
}

#[test]
fn test_snapshot_memory_then_tool_error_order() {
    use std::sync::Arc;

    use coco_messages::AttachmentMessage;
    use coco_messages::LlmMessage;
    use coco_messages::Message;
    use coco_types::AttachmentKind;

    let mut state = AppState::new();
    state.session.model = "opus-4".to_string();
    state.session.working_dir = Some("/repo".to_string());
    test_helpers::push_user_text(&mut state.session, "1", "read the memory-guided file");
    state
        .session
        .transcript
        .on_message_appended(Arc::new(Message::Attachment(AttachmentMessage::api(
            AttachmentKind::NestedMemory,
            LlmMessage::user_text(
                "<system-reminder>\nContents of /repo/CLAUDE.md:\n\n# repo rules\n</system-reminder>",
            ),
        ))));
    test_helpers::push_tool_use_input(
        &mut state.session,
        "read-error-1",
        "Read",
        serde_json::json!({"file_path": "missing.txt"}),
    );
    test_helpers::push_tool_result(
        &mut state.session,
        "read-error-1",
        "Read",
        "File does not exist: missing.txt",
        true,
    );

    let output = render_to_string(&state, 84, 20);
    insta::assert_snapshot!("memory_then_tool_error_order", output);
}

#[test]
fn test_snapshot_edit_diff() {
    // The headline feature: an Edit invocation renders a colored unified diff
    // synthesized from old_string/new_string (not raw text).
    let mut state = AppState::new();
    state.session.model = "opus-4".to_string();
    test_helpers::push_user_text(&mut state.session, "1", "bump the version");
    test_helpers::push_tool_use_input(
        &mut state.session,
        "c1",
        "Edit",
        serde_json::json!({
            "file_path": "Cargo.toml",
            "old_string": "version = \"0.1.0\"\nedition = \"2021\"",
            "new_string": "version = \"0.2.0\"\nedition = \"2024\"",
        }),
    );
    test_helpers::push_tool_result(
        &mut state.session,
        "c1",
        "Edit",
        "updated Cargo.toml",
        false,
    );

    let output = render_to_string(&state, 96, 24);
    insta::assert_snapshot!("edit_diff", output);
}

#[test]
fn test_snapshot_tool_result_middle_truncation() {
    let mut state = AppState::new();
    state.session.model = "opus-4".to_string();
    test_helpers::push_user_text(&mut state.session, "1", "find the CLAUDE.md");
    test_helpers::push_tool_use(&mut state.session, "c1", "Glob", "**/CLAUDE.md");
    // 18 paths exceed the Glob five-row cap so the renderer still exercises
    // the head + "… +N lines" + tail middle-truncation path.
    test_helpers::push_tool_result(
        &mut state.session,
        "c1",
        "Glob",
        "Found 17 files\n\
         coco-rs/app/tui/CLAUDE.md\n\
         coco-rs/app/query/CLAUDE.md\n\
         coco-rs/app/cli/CLAUDE.md\n\
         coco-rs/app/state/CLAUDE.md\n\
         coco-rs/core/tools/CLAUDE.md\n\
         coco-rs/core/messages/CLAUDE.md\n\
         coco-rs/core/permissions/CLAUDE.md\n\
         coco-rs/core/context/CLAUDE.md\n\
         coco-rs/common/config/CLAUDE.md\n\
         coco-rs/common/types/CLAUDE.md\n\
         coco-rs/services/inference/CLAUDE.md\n\
         coco-rs/services/mcp/CLAUDE.md\n\
         coco-rs/utils/string/CLAUDE.md\n\
         coco-rs/utils/git/CLAUDE.md\n\
         coco-rs/vercel-ai/anthropic/CLAUDE.md\n\
         coco-rs/vercel-ai/openai/CLAUDE.md\n\
         codex-rs/tui/CLAUDE.md",
        false,
    );

    let output = render_to_string(&state, 96, 24);
    assert!(output.contains("… +14 lines (ctrl+o to expand)"));
    insta::assert_snapshot!("tool_result_middle_truncation", output);
}

#[test]
fn assistant_text_after_tool_result_keeps_dot_and_full_body() {
    let mut state = AppState::new();
    state.session.model = "opus-4".to_string();
    test_helpers::push_user_text(&mut state.session, "1", "find the readme.md");
    test_helpers::push_tool_use(&mut state.session, "c1", "Glob", "**/README.md");
    test_helpers::push_tool_result(
        &mut state.session,
        "c1",
        "Glob",
        "common/error/README.md\n\
         common/otel/README.md\n\
         core/system-reminder/README.md\n\
         vercel-ai/README.md\n\
         app/tui/README.md\n\
         app/query/README.md\n\
         services/inference/README.md",
        false,
    );
    test_helpers::push_assistant_text(
        &mut state.session,
        "Found 13 `README.md` files in the workspace:\n\n\
         | Path | Description |\n\
         |------|-------------|\n\
         | `common/error/README.md` | Error status codes |\n\
         | `common/otel/README.md` | Tracing conventions |\n\
         | `vercel-ai/README.md` | Provider notes |\n\
         | `app/tui/README.md` | TUI notes |",
    );

    let output = render_to_string(&state, 96, 40);
    assert!(output.contains("⏺ Found 13"));
    assert!(output.contains("common/error/README.md"));
    assert!(output.contains("Error status codes"));
    assert!(!output.contains("… output truncated in UI"));
}

#[test]
fn slash_markdown_result_renders_full_markdown_body() {
    let mut state = AppState::new();
    state.session.model = "opus-4".to_string();
    test_helpers::push_info(
        &mut state.session,
        "",
        "## Context Window Usage\n\n| Category | Tokens |\n|---|---:|\n| Messages | 123 |",
    );

    let output = render_to_string(&state, 80, 24);
    assert!(output.contains("Context Window Usage"));
    assert!(
        output.contains('┌') && output.contains('└'),
        "slash markdown table was not rendered:\n{output}"
    );
    assert!(
        !output.contains("# [system] ## Context Window Usage"),
        "slash markdown result should not collapse to a meta preview:\n{output}"
    );
}

#[test]
fn assistant_fenced_code_renders_with_border_end_to_end() {
    // Proves the pulldown-cmark + syntect renderer (coco-tui-markdown) is wired
    // through app/tui's surface path: a fenced block emits the boxed code-fence
    // frame and preserves the source line.
    let mut state = AppState::new();
    state.session.model = "opus-4".to_string();
    test_helpers::push_assistant_text(
        &mut state.session,
        "Here is some code:\n\n```rust\nfn main() {}\n```",
    );

    let output = render_to_string(&state, 80, 24);
    assert!(output.contains("fn main"), "code body missing:\n{output}");
    assert!(
        output.contains('┌') && output.contains('└'),
        "code-fence border missing — markdown renderer not wired:\n{output}"
    );
}

#[test]
fn assistant_mermaid_fence_renders_as_diagram_end_to_end() {
    // Proves coco-tui-mermaid is reachable through the finalized markdown path:
    // a committed ```mermaid fence lays out as box-drawing cells (labels + box
    // glyphs) rather than the verbatim `flowchart` source.
    let mut state = AppState::new();
    state.session.model = "opus-4".to_string();
    test_helpers::push_assistant_text(
        &mut state.session,
        "```mermaid\nflowchart LR\n  A[Start] --> B[Finish]\n```",
    );

    let output = render_to_string(&state, 96, 40);
    assert!(
        output.contains("Start") && output.contains("Finish"),
        "diagram node labels missing:\n{output}"
    );
    assert!(
        output
            .chars()
            .any(|c| matches!(c, '╭' | '╮' | '╰' | '╯' | '│' | '─')),
        "expected box-drawing cells — mermaid not wired:\n{output}"
    );
    assert!(
        !output.contains("flowchart LR"),
        "fell back to verbatim source instead of a diagram:\n{output}"
    );
}

#[test]
fn test_snapshot_with_permission_prompt() {
    let mut state = AppState::new();
    state.session.model = "opus-4".to_string();
    state
        .ui
        .push_prompt(crate::state::PanePromptState::Permission(
            PermissionPromptState {
                request_id: "r1".to_string(),
                tool_name: "Bash".to_string(),
                description: "Execute shell command".to_string(),
                detail: PermissionDetail::Bash {
                    command: "rm -rf /tmp/test".to_string(),
                    risk_description: Some("Removes files recursively".to_string()),
                    working_dir: Some("/home/user/project".to_string()),
                },
                risk_level: Some(crate::state::RiskLevel::High),
                show_always_allow: true,
                classifier_checking: false,
                classifier_auto_approved: None,
                choices: None,
                selected_choice: 0,
                display_input: coco_types::PermissionDisplayInput::Command(
                    "rm -rf /tmp/test".into(),
                ),
                original_input: None,
                cwd: None,
                permission_suggestions: vec![],
                worker_badge: None,
                explanation_visible: false,
                explanation: crate::state::ExplainerFetch::NotFetched,
                prefix_input: None,
            },
        ));
    mark_retained_surface_visible(&mut state);

    let output = render_to_string(&state, 80, 24);
    insta::assert_snapshot!("permission_prompt", output);
}

#[test]
fn test_snapshot_with_streaming() {
    let mut state = AppState::new();
    state.session.model = "opus-4".to_string();
    test_helpers::push_user_text(&mut state.session, "1", "Explain Rust");

    let mut streaming = StreamingState::new();
    streaming.append_text("Rust is a systems programming language");
    streaming.reveal_all();
    state.ui.streaming = Some(streaming);

    let output = render_to_string(&state, 80, 24);
    insta::assert_snapshot!("streaming", output);
}

#[test]
fn test_snapshot_with_help_modal() {
    let mut state = AppState::new();
    state.session.model = "opus-4".to_string();
    state.ui.show_modal(crate::state::ModalState::Help);

    let output = render_to_string(&state, 80, 24);
    // Modifier label depends on host OS (`opt` on macOS, `alt`
    // elsewhere) — see `coco_keybindings::display::DisplayPlatform`.
    // Per-platform snapshot suffix keeps both labels honest instead
    // of normalizing one away.
    let suffix = if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };
    insta::with_settings!({ snapshot_suffix => suffix }, {
        insta::assert_snapshot!("help_modal", output);
    });
}

#[test]
fn test_snapshot_with_pending_chord() {
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyEventKind;
    use crossterm::event::KeyEventState;
    use crossterm::event::KeyModifiers;

    let mut state = AppState::new();
    state.session.model = "opus-4".to_string();

    // Feed `ctrl+x` (a chord prefix from defaults — `ctrl+x ctrl+k`
    // = chat:killAgents, `ctrl+x ctrl+e` = chat:externalEditor) so
    // the per-state resolver enters Pending. The status bar should
    // then render `"ctrl+x …"` between the permission-mode and token
    // segments.
    let event = KeyEvent {
        code: KeyCode::Char('x'),
        modifiers: KeyModifiers::CONTROL,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    };
    let outcome = state
        .ui
        .kb_handle
        .resolve_key(event, crate::keybinding_bridge::KeybindingContext::Chat);
    // Sanity: resolver must really be in Pending — otherwise the
    // status-bar hint won't render and the snapshot would silently
    // drift.
    assert!(
        matches!(outcome, crate::keybinding_resolver::ResolverResult::Pending),
        "expected Pending, got {outcome:?}",
    );

    let output = render_to_string(&state, 80, 24);
    insta::assert_snapshot!("pending_chord", output);
}

#[test]
fn test_exit_prompt_replaces_status_bar() {
    let mut state = AppState::new();
    state.ui.ctrl_c_tracker.poll((), std::time::Instant::now());

    let output = render_to_string(&state, 80, 24);
    // Status bar lives in the row right under the input — content-flow
    // layout, not at the terminal's last row.
    let status_line = output
        .lines()
        .find(|line| line.contains("Press Ctrl-C again to exit"))
        .unwrap_or_else(|| panic!("exit prompt should render in the status row:\n{output}"));

    assert!(
        !status_line.contains("Default") && !status_line.contains('←'),
        "exit prompt should replace model/mode/message-count status content:\n{output}",
    );
}

#[test]
fn test_snapshot_with_rate_limit_banner() {
    let mut state = AppState::new();
    state.session.model = "opus-4".to_string();
    // Blocking rate-limit: remaining=0 keeps the banner visible.
    state.session.rate_limit_info = Some(crate::state::session::RateLimitInfo {
        remaining: Some(0),
        reset_at: None,
        provider: Some("anthropic".to_string()),
    });

    let output = render_to_string(&state, 80, 24);
    insta::assert_snapshot!("rate_limit_banner", output);
}

#[test]
fn test_snapshot_with_model_fallback_banner() {
    let mut state = AppState::new();
    state.session.model = "sonnet-4-6".to_string();
    state.session.model_fallback_banner = Some("opus-4-7 → sonnet-4-6".to_string());

    let output = render_to_string(&state, 80, 24);
    insta::assert_snapshot!("model_fallback_banner", output);
}

#[test]
fn test_snapshot_with_error_modal() {
    let mut state = AppState::new();
    state.session.model = "opus-4".to_string();
    let body = crate::widgets::error_dialog::format_error_body(
        "Connection refused after 3 retries",
        Some("network"),
        false,
    );
    state.ui.show_modal(crate::state::ModalState::Error(body));
    mark_retained_surface_visible(&mut state);

    let output = render_to_string(&state, 80, 24);
    insta::assert_snapshot!("error_modal_non_retryable", output);
}

#[test]
fn test_snapshot_with_toast() {
    let mut state = AppState::new();
    state.session.model = "opus-4".to_string();
    state.ui.add_toast(Toast::success("Operation completed"));
    state.ui.add_toast(Toast::error("Something went wrong"));

    let output = render_to_string(&state, 80, 24);
    insta::assert_snapshot!("with_toasts", output);
}

#[test]
fn test_snapshot_plan_mode() {
    let mut state = AppState::new();
    state.session.model = "opus-4".to_string();
    state.session.permission_mode = coco_types::PermissionMode::Plan;
    state.session.turn_count = 3;
    test_helpers::push_user_text(&mut state.session, "1", "Plan the implementation");

    let output = render_to_string(&state, 80, 24);
    insta::assert_snapshot!("plan_mode", output);
}

// Edit tool results land as `Message::ToolResult` cells and render
// through the generic tool-result preview — covered by
// `test_snapshot_with_tool_result`.

#[test]
fn test_snapshot_parallel_tool_batch() {
    // Policy A: unresolved tool_use messages stay out of native scrollback
    // until their matching tool results are present.
    let mut state = AppState::new();
    state.session.model = "opus-4".to_string();
    test_helpers::push_user_text(&mut state.session, "1", "Investigate");
    for (i, (name, input)) in [
        ("Bash", "ls -la"),
        ("Read", "src/main.rs"),
        ("Grep", "TODO"),
    ]
    .iter()
    .enumerate()
    {
        test_helpers::push_tool_use(&mut state.session, &format!("c{i}"), name, input);
    }

    let output = render_to_string(&state, 80, 24);
    insta::assert_snapshot!("parallel_tool_batch", output);
}

#[test]
fn test_snapshot_completed_parallel_tool_batch() {
    // Spec: crate-coco-tui.md §ChatWidget Internals — completed parallel
    // tool_use messages are rendered under a `‖ N in parallel` header and each
    // result is paired to the matching call_id.
    let mut state = AppState::new();
    state.session.model = "opus-4".to_string();
    test_helpers::push_user_text(&mut state.session, "1", "Investigate");
    let tools = [
        ("Bash", "ls -la", "Cargo.toml\nsrc"),
        ("Glob", "**/*.rs", "src/main.rs\nsrc/lib.rs"),
    ];
    for (i, (name, input, _output)) in tools.iter().enumerate() {
        let call_id = format!("c{i}");
        test_helpers::push_tool_use(&mut state.session, &call_id, name, input);
    }
    for (i, (name, _input, output)) in tools.iter().enumerate() {
        let call_id = format!("c{i}");
        test_helpers::push_tool_result(&mut state.session, &call_id, name, output, false);
    }

    let output = render_to_string(&state, 80, 24);
    insta::assert_snapshot!("completed_parallel_tool_batch", output);
}

#[test]
fn test_snapshot_subagent_panel_populated() {
    // P2 / A5: subagent panel must render mixed status states
    // (Running/Completed/Failed/Backgrounded) without truncating
    // the description or losing the focus indicator.
    use crate::state::session::SubagentInstance;
    use crate::state::session::SubagentStatus;
    let mut state = AppState::new();
    state.session.model = "opus-4".to_string();
    state.session.subagents = vec![
        SubagentInstance {
            kind: crate::state::session::SubagentKind::Subagent,
            agent_id: "agent-7af2".into(),
            agent_type: "Explore".into(),
            description: "Search for auth handlers".into(),
            status: SubagentStatus::Running,
            color: Some(coco_types::AgentColorName::Blue),
            team_name: None,
            started_at_ms: None,
            last_tool_name: None,
            tool_count: 0,
            total_tokens: 0,
            is_backgrounded: false,
            recent_activities: Vec::new(),
            final_message: None,
            completed_at_ms: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cost_usd: 0.0,
        },
        SubagentInstance {
            kind: crate::state::session::SubagentKind::Subagent,
            agent_id: "agent-c1d3".into(),
            agent_type: "Plan".into(),
            description: "Design refactor plan".into(),
            // Running so the populated panel shows multiple live rows; the
            // Failed agent below is terminal and must drop out (transient
            // stage view — completed agents commit to the transcript).
            status: SubagentStatus::Running,
            color: None,
            team_name: None,
            started_at_ms: None,
            last_tool_name: None,
            tool_count: 0,
            total_tokens: 0,
            is_backgrounded: false,
            recent_activities: Vec::new(),
            final_message: None,
            completed_at_ms: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cost_usd: 0.0,
        },
        SubagentInstance {
            kind: crate::state::session::SubagentKind::Subagent,
            agent_id: "agent-9e5f".into(),
            agent_type: "verification".into(),
            description: "Run tests + verify".into(),
            status: SubagentStatus::Failed,
            color: Some(coco_types::AgentColorName::Red),
            team_name: None,
            started_at_ms: None,
            last_tool_name: None,
            tool_count: 0,
            total_tokens: 0,
            is_backgrounded: false,
            recent_activities: Vec::new(),
            final_message: None,
            completed_at_ms: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cost_usd: 0.0,
        },
    ];
    state.session.focused_subagent_index = Some(0);
    let output = render_to_string(&state, 100, 24);
    insta::assert_snapshot!("subagent_panel_populated", output);
}

#[test]
fn test_snapshot_subagent_panel_live_cost() {
    // The header shows `completed/total` for the current wave (a finished
    // sibling stays counted so the fraction climbs), and per-row + header
    // carry live token + cost from the child's per-round usage snapshot.
    use crate::state::session::SubagentInstance;
    use crate::state::session::SubagentKind;
    use crate::state::session::SubagentStatus;
    const NOW: i64 = 1_000_000_000;
    let mut state = AppState::with_clock(coco_tui_ui::clock::MockClock::arc(NOW));
    state.session.model = "opus-4".to_string();
    let row = |id: &str,
               agent_type: &str,
               description: &str,
               status: SubagentStatus,
               started_at_ms: Option<i64>,
               completed_at_ms: Option<i64>,
               tool_count: i32,
               input_tokens: i64,
               output_tokens: i64,
               cache_read_tokens: i64,
               cost_usd: f64| SubagentInstance {
        kind: SubagentKind::Subagent,
        agent_id: id.into(),
        agent_type: agent_type.into(),
        description: description.into(),
        status,
        color: None,
        team_name: None,
        started_at_ms,
        last_tool_name: None,
        tool_count,
        total_tokens: input_tokens + output_tokens,
        is_backgrounded: false,
        recent_activities: Vec::new(),
        final_message: None,
        completed_at_ms,
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cost_usd,
    };
    state.session.subagents = vec![
        // Running, with live spend mid-run.
        row(
            "agent-a",
            "Explore",
            "Map app/root crates",
            SubagentStatus::Running,
            Some(NOW - 88_000),
            None,
            33,
            68_100,
            468,
            64_000,
            0.12,
        ),
        // Finished sibling — drops from the rows but stays in the wave count
        // and the header token/cost aggregate.
        row(
            "agent-b",
            "Explore",
            "Map common crates",
            SubagentStatus::Completed,
            Some(NOW - 90_000),
            Some(NOW - 5_000),
            37,
            70_000,
            500,
            66_000,
            0.18,
        ),
        row(
            "agent-c",
            "Plan",
            "Design refactor",
            SubagentStatus::Running,
            Some(NOW - 80_000),
            None,
            10,
            12_000,
            100,
            0,
            0.01,
        ),
    ];
    let output = render_to_string(&state, 100, 24);
    insta::assert_snapshot!("subagent_panel_live_cost", output);
}

#[test]
fn test_snapshot_prompt_suggestion_renders_as_dim_placeholder() {
    // P5 / A4: when input is empty AND a prompt suggestion is
    // available, the suggestion replaces the default placeholder.
    let mut state = AppState::new();
    state.session.model = "opus-4".to_string();
    state.session.prompt_suggestions = vec!["Run the failing tests".into()];
    let output = render_to_string(&state, 80, 24);
    insta::assert_snapshot!("prompt_suggestion_placeholder", output);
}

#[test]
fn test_snapshot_autocomplete_popup() {
    // Spec: crate-coco-tui.md §Autocomplete Systems — slash command
    // trigger populates the popup synchronously from session state.
    let mut state = AppState::new();
    state.session.model = "opus-4".to_string();
    state.session.available_commands = vec![
        crate::state::SlashCommandInfo {
            name: "help".into(),
            description: Some("Show help".into()),
            ..Default::default()
        },
        crate::state::SlashCommandInfo {
            name: "clear".into(),
            description: Some("Clear chat".into()),
            ..Default::default()
        },
        crate::state::SlashCommandInfo {
            name: "config".into(),
            description: Some("Settings".into()),
            ..Default::default()
        },
    ];
    state.ui.input.textarea.set_text("/c");
    state.ui.input.textarea.set_cursor(2);
    crate::autocomplete::refresh_suggestions(&mut state);

    let output = render_to_string(&state, 80, 24);
    insta::assert_snapshot!("autocomplete_popup", output);
}

#[test]
fn test_snapshot_command_palette_inline_popup() {
    // Spec: Ctrl+P opens the command palette as a borderless list floated
    // above the input. Typed chars route to `cp.filter` and the input bar
    // mirrors `/<filter>` so the user can see what they typed.
    let mut state = AppState::new();
    state.session.model = "opus-4".to_string();
    state.ui.input.textarea.set_text("/c");
    state.ui.input.textarea.set_cursor(2);
    state.ui.completion.active = Some(crate::state::ActiveSuggestions {
        kind: SuggestionKind::SlashCommand,
        items: vec![
            crate::widgets::suggestion_popup::SuggestionItem {
                label: "/clear".into(),
                description: Some("Clear conversation".into()),
                metadata: None,
            },
            crate::widgets::suggestion_popup::SuggestionItem {
                label: "/compact".into(),
                description: Some("Compact conversation".into()),
                metadata: None,
            },
        ],
        selected: 0,
        query: "c".into(),
        trigger_pos: 0,
    });
    state.ui.sync_popup_from_active_suggestions();

    let output = render_to_string(&state, 80, 24);
    insta::assert_snapshot!("command_palette_inline", output);
}

#[test]
fn test_snapshot_reverse_search_footer_match() {
    // Ctrl+R reverse-i-search previewing a match: the composer shows the
    // matched history entry and the bottom border carries the
    // `reverse-i-search: <query>` footer plus the accept/cancel hint.
    let mut state = AppState::new();
    state.ui.input.add_to_history("git status".to_string());
    state.ui.input.add_to_history("cargo build".to_string());
    state.ui.input.textarea.set_text("git status");
    state.ui.history_search = Some(crate::state::HistorySearch {
        query: "git".into(),
        matched: Some(1),
        original_text: String::new(),
        original_pastes: Vec::new(),
        original_history_index: None,
    });

    let output = render_to_string(&state, 80, 24);
    insta::assert_snapshot!("reverse_search_footer_match", output);
}

#[test]
fn test_snapshot_reverse_search_footer_no_match() {
    // No history entry matches the query: the composer keeps the draft and
    // the footer shows the `no match` warning instead of the accept hint.
    let mut state = AppState::new();
    state.ui.input.add_to_history("git status".to_string());
    state.ui.input.textarea.set_text("draft text");
    state.ui.history_search = Some(crate::state::HistorySearch {
        query: "zzz".into(),
        matched: None,
        original_text: "draft text".into(),
        original_pastes: Vec::new(),
        original_history_index: None,
    });

    let output = render_to_string(&state, 80, 24);
    insta::assert_snapshot!("reverse_search_footer_no_match", output);
}
