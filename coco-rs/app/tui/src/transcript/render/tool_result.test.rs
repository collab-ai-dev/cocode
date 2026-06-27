use super::*;
use coco_tui_ui::style::UiStyles;
use coco_tui_ui::theme::Theme;
use coco_types::ApplyPatchPreview;
use coco_types::ApplyPatchPreviewAction;
use coco_types::ApplyPatchPreviewRow;
use coco_types::ApplyPatchPreviewSign;
use coco_types::AskUserQuestionAnswered;
use coco_types::AskUserQuestionResult;
use coco_types::ExitPlanModeResult;
use coco_types::ToolDisplayData;
use serde_json::json;

/// Flatten rendered lines to a single string for content assertions.
fn text_of(lines: &[Line<'static>]) -> String {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render with an explicit `expanded` flag (the reader surface relaxes caps).
fn render_ex(
    tool_name: &str,
    input: Option<Value>,
    output: &str,
    is_error: bool,
    expanded: bool,
) -> Vec<Line<'static>> {
    render_ex_width(
        tool_name, input, output, is_error, expanded, /*width*/ 96,
    )
}

fn render_ex_width(
    tool_name: &str,
    input: Option<Value>,
    output: &str,
    is_error: bool,
    expanded: bool,
    width: u16,
) -> Vec<Line<'static>> {
    render_ex_width_with_display(tool_name, input, output, None, is_error, expanded, width)
}

fn render_ex_width_with_display(
    tool_name: &str,
    input: Option<Value>,
    output: &str,
    display_data: Option<&ToolDisplayData>,
    is_error: bool,
    expanded: bool,
    width: u16,
) -> Vec<Line<'static>> {
    let theme = Theme::default();
    let cx = ToolResultRenderCtx {
        styles: UiStyles::new(&theme),
        width,
        syntax_highlighting: SyntaxHighlighting::Enabled,
        plan_editor_hint: "ctrl+g to edit".to_string(),
        expand_hint: "(ctrl+o to expand)".to_string(),
        expanded,
    };
    let mut lines = Vec::new();
    render_tool_result_body(
        &cx,
        tool_name,
        input.as_ref(),
        output,
        display_data,
        is_error,
        &mut lines,
    );
    lines
}

/// Inline-surface render (caps tight, hint points at the reader).
fn render(
    tool_name: &str,
    input: Option<Value>,
    output: &str,
    is_error: bool,
) -> Vec<Line<'static>> {
    render_ex(tool_name, input, output, is_error, /*expanded*/ false)
}

fn apply_patch_display_data(rows: Vec<ApplyPatchPreviewRow>) -> ToolDisplayData {
    ToolDisplayData::ApplyPatchPreview(ApplyPatchPreview { rows })
}

fn exit_plan_display_data(
    outcome: coco_types::ExitPlanModeOutcome,
    plan: &str,
    file_path: Option<&str>,
    awaiting_leader_approval: bool,
) -> ToolDisplayData {
    ToolDisplayData::ExitPlanModeResult(ExitPlanModeResult {
        outcome,
        plan: plan.to_string(),
        file_path: file_path.map(str::to_string),
        awaiting_leader_approval,
        is_agent: false,
        plan_was_edited: false,
    })
}

fn render_apply_patch(rows: Vec<ApplyPatchPreviewRow>, is_error: bool) -> Vec<Line<'static>> {
    let display_data = apply_patch_display_data(rows);
    render_ex_width_with_display(
        "apply_patch",
        None,
        if is_error { "Error: failed" } else { "Done!" },
        Some(&display_data),
        is_error,
        /*expanded*/ false,
        /*width*/ 96,
    )
}

fn render_apply_patch_width(
    rows: Vec<ApplyPatchPreviewRow>,
    is_error: bool,
    width: u16,
) -> Vec<Line<'static>> {
    let display_data = apply_patch_display_data(rows);
    render_ex_width_with_display(
        "apply_patch",
        None,
        if is_error { "Error: failed" } else { "Done!" },
        Some(&display_data),
        is_error,
        /*expanded*/ false,
        width,
    )
}

#[test]
fn unified_diff_text_emits_hunk_and_signed_lines() {
    let diff = unified_diff_text("let x = 1;\n", "let x = 2;\n");
    assert!(diff.contains("@@"), "expected a hunk header: {diff}");
    assert!(
        diff.contains("-let x = 1;"),
        "expected removed line: {diff}"
    );
    assert!(diff.contains("+let x = 2;"), "expected added line: {diff}");
}

#[test]
fn edit_renders_a_diff_from_old_new_input() {
    let input =
        json!({"file_path": "a.rs", "old_string": "let x = 1;", "new_string": "let x = 2;"});
    let out = text_of(&render("Edit", Some(input), "applied", false));
    assert!(out.contains("let x = 1;"), "old content must appear: {out}");
    assert!(out.contains("let x = 2;"), "new content must appear: {out}");
}

#[test]
fn edit_diff_preview_caps_wrapped_screen_rows() {
    let old = (0..40)
        .map(|i| format!("let old_{i} = \"{}\";", "long_segment_".repeat(8)))
        .collect::<Vec<_>>()
        .join("\n");
    let new = (0..40)
        .map(|i| format!("let new_{i} = \"{}\";", "long_segment_".repeat(8)))
        .collect::<Vec<_>>()
        .join("\n");
    let input = json!({"file_path": "a.rs", "old_string": old, "new_string": new});
    let lines = render_ex_width(
        "Edit",
        Some(input),
        "applied",
        false,
        /*expanded*/ false,
        /*width*/ 32,
    );
    let out = text_of(&lines);

    assert!(lines.len() <= 24, "inline diff rows must be capped: {out}");
    assert!(out.contains("… +"), "diff should include truncation: {out}");
}

#[test]
fn edit_without_input_falls_back_to_output() {
    // Standalone path (input: None) must not panic and shows the tool output.
    let out = text_of(&render("Edit", None, "edited 1 file", false));
    assert!(out.contains("edited 1 file"), "{out}");
}

#[test]
fn bash_renders_output_without_echoing_the_command() {
    // The `●` header already names the command; the body shows only output,
    // never a redundant `$ command` line.
    let input = json!({"command": "ls -la /tmp"});
    let out = text_of(&render("Bash", Some(input), "file_a\nfile_b", false));
    assert!(out.contains("file_a"), "output expected: {out}");
    assert!(
        !out.contains("$ ls -la"),
        "command must not be echoed in the body: {out}"
    );
}

#[test]
fn todowrite_renders_status_glyphs() {
    let input = json!({"todos": [
        {"content": "done item", "status": "completed"},
        {"content": "active item", "status": "in_progress"},
        {"content": "todo item", "status": "pending"},
    ]});
    let out = text_of(&render("TodoWrite", Some(input), "ok", false));
    assert!(out.contains("✔ done item"), "{out}");
    assert!(out.contains("◐ active item"), "{out}");
    assert!(out.contains("☐ todo item"), "{out}");
}

#[test]
fn read_highlights_file_content() {
    // Routed through the markdown code highlighter — the content survives.
    let input = json!({"file_path": "main.rs"});
    let out = text_of(&render(
        "Read",
        Some(input),
        "fn main() { println!(\"hi\"); }",
        false,
    ));
    assert!(out.contains("fn main()"), "file content must render: {out}");
}

#[test]
fn read_splits_cat_n_line_numbers_into_a_gutter() {
    // Read returns `cat -n` (`<n>\t<content>`). The line number must land in a
    // dim gutter, never jammed against the content as `1# heading`.
    let input = json!({"file_path": "README.md"});
    let out = text_of(&render(
        "Read",
        Some(input),
        "1\t# heading\n2\t\n3\tbody text",
        false,
    ));
    assert!(
        out.contains("# heading"),
        "markdown content must survive line-number stripping: {out}"
    );
    assert!(
        out.contains("body text"),
        "later lines must render too: {out}"
    );
    assert!(
        !out.contains("1# heading"),
        "line number must not jam against content: {out}"
    );
    assert!(
        !out.contains('\t'),
        "the cat -n tab must be consumed, not emitted: {out}"
    );
}

#[test]
fn read_preview_uses_a_single_trailing_ellipsis() {
    // A read longer than the inline cap collapses to a contiguous head plus ONE
    // trailing "… +N lines" marker — never a stacked middle ellipsis whose count
    // disagrees with the line-number gutter (regression for the cat -n preview).
    let input = json!({"file_path": "README.md"});
    let output = (1..=10)
        .map(|n| format!("{n}\tline {n} content"))
        .collect::<Vec<_>>()
        .join("\n");
    let out = text_of(&render("Read", Some(input), &output, false));
    assert_eq!(
        out.matches("… +").count(),
        1,
        "exactly one truncation marker expected: {out}"
    );
    // Head is contiguous (lines 1..=5 shown), the rest collapse into the marker.
    assert!(
        out.contains("5  line 5 content"),
        "head line 5 shown: {out}"
    );
    assert!(
        !out.contains("6  line 6 content"),
        "line 6 must be elided into the marker, not shown: {out}"
    );
    assert!(
        out.contains("… +5 lines"),
        "marker must count the 5 omitted lines (6-10): {out}"
    );
}

#[test]
fn read_code_truncates_by_wrapped_screen_rows() {
    let input = json!({"file_path": "main.rs"});
    let long = "let value = \"".to_string() + &"very_long_segment_".repeat(30) + "\";";
    let output = format!("{long}\n{long}");
    let out = text_of(&render_ex_width(
        "Read",
        Some(input),
        &output,
        false,
        /*expanded*/ false,
        /*width*/ 24,
    ));

    assert!(
        out.contains("… +"),
        "wrapped long code should collapse to an ellipsis: {out}"
    );
}

#[test]
fn structured_default_pretty_prints_json() {
    // A tool with no bespoke renderer + JSON output → multi-line pretty print.
    let out = text_of(&render(
        "Config",
        None,
        r#"{"theme":"dark","width":80}"#,
        false,
    ));
    assert!(
        out.contains("\"theme\": \"dark\""),
        "expected pretty JSON: {out}"
    );
    assert!(
        out.lines().count() > 1,
        "pretty JSON must be multi-line: {out}"
    );
}

#[test]
fn exit_plan_mode_empty_plan_renders_inline_exit_notice_from_display_data() {
    let display = exit_plan_display_data(
        coco_types::ExitPlanModeOutcome::NoImplementationPlan,
        "",
        None,
        false,
    );
    let out = text_of(&render_ex_width_with_display(
        "ExitPlanMode",
        None,
        r#"{"plan":""}"#,
        Some(&display),
        /*is_error*/ false,
        /*expanded*/ false,
        /*width*/ 96,
    ));

    assert!(out.contains("Exited plan mode"), "{out}");
    assert!(!out.contains("\"plan\""), "must not dump raw JSON: {out}");
}

#[test]
fn exit_plan_mode_no_plan_notice_does_not_render_saved_path() {
    let display = exit_plan_display_data(
        coco_types::ExitPlanModeOutcome::NoImplementationPlan,
        "User asked for a read-only explanation.",
        Some("/tmp/session-plan.md"),
        false,
    );
    let out = text_of(&render_ex_width_with_display(
        "ExitPlanMode",
        None,
        "model-facing instructions are ignored",
        Some(&display),
        /*is_error*/ false,
        /*expanded*/ false,
        /*width*/ 96,
    ));

    assert!(out.contains("Exited plan mode"), "{out}");
    assert!(!out.contains("User approved plan"), "{out}");
    assert!(!out.contains("Plan saved to:"), "{out}");
    assert!(!out.contains("to edit"), "{out}");
    assert!(!out.contains("read-only explanation"), "{out}");
}

#[test]
fn exit_plan_mode_with_plan_renders_approval_and_plan_from_display_data() {
    let display = exit_plan_display_data(
        coco_types::ExitPlanModeOutcome::ImplementationPlan,
        "# Plan\n- Edit the renderer\n- Add tests",
        Some("/tmp/session-plan.md"),
        false,
    );
    let out = text_of(&render_ex_width_with_display(
        "ExitPlanMode",
        None,
        "model-facing instructions are ignored",
        Some(&display),
        /*is_error*/ false,
        /*expanded*/ false,
        /*width*/ 96,
    ));

    assert!(out.contains("User approved plan"), "{out}");
    assert!(
        out.contains("Plan saved to: /tmp/session-plan.md · ctrl+g to edit"),
        "{out}"
    );
    // The plan is rendered as markdown, so the body text survives (the `#` /
    // `-` syntax markers are consumed by the renderer).
    assert!(out.contains("Edit the renderer"), "{out}");
    assert!(out.contains("Add tests"), "{out}");
    assert!(
        !out.contains("model-facing instructions"),
        "must not show model-only tool result prose: {out}"
    );
}

#[test]
fn exit_plan_mode_renders_full_plan_without_row_cap() {
    // The approved plan is the durable record — it must NOT be truncated to the
    // structured-preview row cap the ephemeral approval panel uses.
    let plan: String = (1..=40).map(|i| format!("- step {i}\n")).collect();
    let display = exit_plan_display_data(
        coco_types::ExitPlanModeOutcome::ImplementationPlan,
        &plan,
        None,
        false,
    );
    let out = text_of(&render_ex_width_with_display(
        "ExitPlanMode",
        None,
        "ignored",
        Some(&display),
        /*is_error*/ false,
        /*expanded*/ false,
        /*width*/ 96,
    ));

    assert!(out.contains("step 1"), "first step missing: {out}");
    assert!(
        out.contains("step 40"),
        "last step must survive (no cap): {out}"
    );
    assert!(
        !out.contains("+") || !out.contains("lines"),
        "must not emit a truncation marker: {out}"
    );
}

#[test]
fn exit_plan_mode_awaiting_leader_approval_renders_pending_notice() {
    let display = exit_plan_display_data(
        coco_types::ExitPlanModeOutcome::ImplementationPlan,
        "# Plan\n- Wait for lead",
        Some("/tmp/agent-plan.md"),
        true,
    );
    let out = text_of(&render_ex_width_with_display(
        "ExitPlanMode",
        None,
        "model-facing instructions are ignored",
        Some(&display),
        /*is_error*/ false,
        /*expanded*/ false,
        /*width*/ 96,
    ));

    assert!(
        out.contains("Plan submitted for team lead approval"),
        "{out}"
    );
    assert!(out.contains("Plan saved to: /tmp/agent-plan.md"), "{out}");
    assert!(
        !out.contains("to edit"),
        "teammate approval pending must not show local edit hint: {out}"
    );
    assert!(out.contains("Wait for lead"), "{out}");
    assert!(
        !out.contains("model-facing instructions"),
        "must not show model-only tool result prose: {out}"
    );
}

#[test]
fn exit_plan_mode_without_display_data_falls_back_to_output() {
    let out = text_of(&render(
        "ExitPlanMode",
        None,
        "legacy output fallback",
        false,
    ));

    assert!(out.contains("legacy output fallback"), "{out}");
    assert!(
        !out.contains("Exited plan mode"),
        "missing display data must not fabricate an empty-plan exit: {out}"
    );
}

#[test]
fn mcp_tool_name_routes_to_structured_default_without_panicking() {
    // `mcp__server__tool` does not parse to a ToolName → structured default.
    let out = text_of(&render("mcp__slack__send", None, r#"{"ok":true}"#, false));
    assert!(
        out.contains("\"ok\": true"),
        "MCP JSON pretty-printed: {out}"
    );
}

#[test]
fn error_output_renders_regardless_of_tool() {
    let out = text_of(&render(
        "Bash",
        Some(json!({"command": "boom"})),
        "command failed: boom",
        true,
    ));
    assert!(out.contains("command failed: boom"), "{out}");
}

#[test]
fn ask_user_question_renders_styled_answered_cell() {
    let display = ToolDisplayData::AskUserQuestionResult(AskUserQuestionResult {
        questions: vec![
            AskUserQuestionAnswered {
                question: "Pick a library?".into(),
                answers: vec!["date-fns".into()],
                note: None,
            },
            AskUserQuestionAnswered {
                question: "Which features?".into(),
                answers: vec!["i18n".into(), "timezones".into()],
                note: Some("plus DST".into()),
            },
            AskUserQuestionAnswered {
                question: "Anything else?".into(),
                answers: vec![],
                note: None,
            },
        ],
    });
    let out = text_of(&render_ex_width_with_display(
        "AskUserQuestion",
        None,
        "ignored model prose",
        Some(&display),
        false,
        false,
        80,
    ));
    assert!(out.contains("Questions 2/3 answered"), "{out}");
    assert!(out.contains("• Pick a library?"), "{out}");
    assert!(out.contains("answer: date-fns"), "{out}");
    assert!(out.contains("answer: i18n"), "{out}");
    assert!(out.contains("answer: timezones"), "{out}");
    assert!(out.contains("note: plus DST"), "{out}");
    assert!(out.contains("Anything else? (unanswered)"), "{out}");
    // The raw model-facing prose must NOT leak into the styled cell.
    assert!(!out.contains("ignored model prose"), "{out}");
}

#[test]
fn ask_user_question_without_display_data_falls_back_to_prose() {
    let out = text_of(&render_ex_width_with_display(
        "AskUserQuestion",
        None,
        "User has answered your questions: \"Q\"=\"A\".",
        None,
        false,
        false,
        80,
    ));
    assert!(out.contains("User has answered your questions"), "{out}");
}

#[test]
fn apply_patch_add_file_renders_path_header_and_added_lines() {
    let out = text_of(&render_apply_patch(
        vec![
            ApplyPatchPreviewRow::Header {
                action: ApplyPatchPreviewAction::Add,
                target: "src/new.rs".to_string(),
            },
            ApplyPatchPreviewRow::Line {
                sign: ApplyPatchPreviewSign::Added,
                content: "fn main() {}".to_string(),
            },
            ApplyPatchPreviewRow::Line {
                sign: ApplyPatchPreviewSign::Added,
                content: "println!(\"hi\");".to_string(),
            },
        ],
        false,
    ));

    assert!(out.contains("add src/new.rs"), "{out}");
    assert!(out.contains("+fn main() {}"), "{out}");
    assert!(out.contains("+println!(\"hi\");"), "{out}");
}

#[test]
fn apply_patch_update_file_renders_diff_not_raw_patch_markers() {
    let out = text_of(&render_apply_patch(
        vec![
            ApplyPatchPreviewRow::Header {
                action: ApplyPatchPreviewAction::Update,
                target: "src/lib.rs".to_string(),
            },
            ApplyPatchPreviewRow::Line {
                sign: ApplyPatchPreviewSign::Removed,
                content: "old line".to_string(),
            },
            ApplyPatchPreviewRow::Line {
                sign: ApplyPatchPreviewSign::Added,
                content: "new line".to_string(),
            },
        ],
        false,
    ));

    assert!(out.contains("update src/lib.rs"), "{out}");
    assert!(out.contains("-old line"), "{out}");
    assert!(out.contains("+new line"), "{out}");
    assert!(
        !out.contains("@@ -"),
        "apply_patch previews must not invent source hunk line numbers: {out}"
    );
    assert!(
        !out.contains("*** Update File"),
        "raw patch marker should not render after parse: {out}"
    );
}

#[test]
fn apply_patch_move_file_shows_source_and_destination() {
    let out = text_of(&render_apply_patch(
        vec![
            ApplyPatchPreviewRow::Header {
                action: ApplyPatchPreviewAction::Update,
                target: "old.rs -> new.rs".to_string(),
            },
            ApplyPatchPreviewRow::Line {
                sign: ApplyPatchPreviewSign::Removed,
                content: "old_name()".to_string(),
            },
            ApplyPatchPreviewRow::Line {
                sign: ApplyPatchPreviewSign::Added,
                content: "new_name()".to_string(),
            },
        ],
        false,
    ));

    assert!(out.contains("update old.rs -> new.rs"), "{out}");
    assert!(out.contains("-old_name()"), "{out}");
    assert!(out.contains("+new_name()"), "{out}");
}

#[test]
fn apply_patch_delete_file_renders_delete_header_only() {
    let out = text_of(&render_apply_patch(
        vec![ApplyPatchPreviewRow::Header {
            action: ApplyPatchPreviewAction::Delete,
            target: "obsolete.rs".to_string(),
        }],
        false,
    ));

    assert!(out.contains("delete obsolete.rs"), "{out}");
    assert!(
        !out.contains("-obsolete"),
        "delete body is unavailable: {out}"
    );
}

#[test]
fn apply_patch_malformed_patch_falls_back_to_raw_signed_lines() {
    let out = text_of(&render_apply_patch(
        vec![
            ApplyPatchPreviewRow::Raw {
                content: "*** Update File: src/lib.rs".to_string(),
            },
            ApplyPatchPreviewRow::Line {
                sign: ApplyPatchPreviewSign::Removed,
                content: "old line".to_string(),
            },
            ApplyPatchPreviewRow::Line {
                sign: ApplyPatchPreviewSign::Added,
                content: "new line".to_string(),
            },
        ],
        false,
    ));

    assert!(out.contains("*** Update File: src/lib.rs"), "{out}");
    assert!(out.contains("-old line"), "{out}");
    assert!(out.contains("+new line"), "{out}");
}

#[test]
fn apply_patch_large_preview_stays_capped() {
    let rows = std::iter::once(ApplyPatchPreviewRow::Header {
        action: ApplyPatchPreviewAction::Add,
        target: "big.rs".to_string(),
    })
    .chain((0..80).map(|i| ApplyPatchPreviewRow::Line {
        sign: ApplyPatchPreviewSign::Added,
        content: format!("let line_{i} = \"{}\";", "long_segment_".repeat(8)),
    }))
    .collect::<Vec<_>>();
    let lines = render_apply_patch_width(rows, false, /*width*/ 32);
    let out = text_of(&lines);

    assert!(
        lines.len() <= 24,
        "inline apply_patch rows must be globally capped: {out}"
    );
    assert!(
        out.contains("… +"),
        "large diff should include truncation: {out}"
    );
}

#[test]
fn apply_patch_many_file_preview_stays_globally_capped() {
    let rows = (0..40)
        .flat_map(|i| {
            [
                ApplyPatchPreviewRow::Header {
                    action: ApplyPatchPreviewAction::Add,
                    target: format!("file_{i}.rs"),
                },
                ApplyPatchPreviewRow::Line {
                    sign: ApplyPatchPreviewSign::Added,
                    content: format!("line {i}"),
                },
            ]
        })
        .collect();
    let lines = render_apply_patch_width(rows, false, /*width*/ 48);
    let out = text_of(&lines);

    assert!(
        lines.len() <= 24,
        "multi-file apply_patch rows must share one cap: {out}"
    );
    assert!(out.contains("add file_0.rs"), "{out}");
    assert!(out.contains("add file_39.rs"), "{out}");
    assert!(out.contains("… +"), "{out}");
}

#[test]
fn apply_patch_preview_uses_core_omission_count() {
    let rows = vec![
        ApplyPatchPreviewRow::Header {
            action: ApplyPatchPreviewAction::Add,
            target: "first.rs".to_string(),
        },
        ApplyPatchPreviewRow::Line {
            sign: ApplyPatchPreviewSign::Added,
            content: "head".to_string(),
        },
        ApplyPatchPreviewRow::Omitted { rows: 37 },
        ApplyPatchPreviewRow::Header {
            action: ApplyPatchPreviewAction::Add,
            target: "last.rs".to_string(),
        },
        ApplyPatchPreviewRow::Line {
            sign: ApplyPatchPreviewSign::Added,
            content: "tail".to_string(),
        },
    ];
    let out = text_of(&render_apply_patch(rows, false));

    assert!(out.contains("add first.rs"), "{out}");
    assert!(out.contains("add last.rs"), "{out}");
    assert!(out.contains("… +37 lines"), "{out}");
}

#[test]
fn apply_patch_second_pass_keeps_surviving_omitted_count() {
    let mut rows = vec![
        ApplyPatchPreviewRow::Header {
            action: ApplyPatchPreviewAction::Add,
            target: "first.rs".to_string(),
        },
        ApplyPatchPreviewRow::Line {
            sign: ApplyPatchPreviewSign::Added,
            content: "head".to_string(),
        },
        ApplyPatchPreviewRow::Omitted { rows: 37 },
    ];
    rows.extend((0..40).map(|i| ApplyPatchPreviewRow::Line {
        sign: ApplyPatchPreviewSign::Added,
        content: format!("line {i}"),
    }));

    let out = text_of(&render_apply_patch_width(rows, false, /*width*/ 96));

    assert!(out.contains("… +37 lines"), "{out}");
    assert!(out.contains("… +20 lines"), "{out}");
}

#[test]
fn apply_patch_second_pass_counts_dropped_omitted_row() {
    let rows = (0..40)
        .map(|i| {
            if i == 20 {
                ApplyPatchPreviewRow::Omitted { rows: 37 }
            } else {
                ApplyPatchPreviewRow::Line {
                    sign: ApplyPatchPreviewSign::Added,
                    content: format!("line {i}"),
                }
            }
        })
        .collect();

    let out = text_of(&render_apply_patch_width(rows, false, /*width*/ 96));

    assert!(out.contains("… +53 lines"), "{out}");
    assert!(!out.contains("… +17 lines"), "{out}");
    assert!(!out.contains("… +37 lines"), "{out}");
}

#[test]
fn apply_patch_error_renders_preview_then_error_text() {
    let out = text_of(&render_apply_patch(
        vec![
            ApplyPatchPreviewRow::Header {
                action: ApplyPatchPreviewAction::Update,
                target: "src/lib.rs".to_string(),
            },
            ApplyPatchPreviewRow::Line {
                sign: ApplyPatchPreviewSign::Removed,
                content: "old".to_string(),
            },
        ],
        true,
    ));

    assert!(out.contains("update src/lib.rs"), "{out}");
    assert!(out.contains("-old"), "{out}");
    assert!(out.contains("Error: failed"), "{out}");
}

#[test]
fn non_apply_patch_error_ignores_apply_patch_preview_display_data() {
    let display_data = apply_patch_display_data(vec![ApplyPatchPreviewRow::Header {
        action: ApplyPatchPreviewAction::Update,
        target: "src/lib.rs".to_string(),
    }]);
    let out = text_of(&render_ex_width_with_display(
        "Bash",
        None,
        "Error: failed",
        Some(&display_data),
        /*is_error*/ true,
        /*expanded*/ false,
        /*width*/ 96,
    ));

    assert!(!out.contains("update src/lib.rs"), "{out}");
    assert!(out.contains("Error: failed"), "{out}");
}

#[test]
fn expanded_surface_shows_more_rows_than_inline() {
    // The headline contract: a body the inline surface truncates ("… ctrl+o to
    // expand") resolves to its fuller form on the reader surface (`expanded`).
    let big = (0..200)
        .map(|i| format!("match line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let inline = render_ex("Grep", None, &big, false, /*expanded*/ false);
    let reader = render_ex("Grep", None, &big, false, /*expanded*/ true);
    assert!(
        reader.len() > inline.len(),
        "reader must show more rows than inline: inline={} reader={}",
        inline.len(),
        reader.len()
    );
}

#[test]
fn plain_output_truncates_by_wrapped_screen_rows() {
    let long = "https://example.test/".to_string() + &"very-long-segment/".repeat(30);
    let output = format!("{long}\n{long}");
    let out = text_of(&render_ex_width(
        "Bash",
        Some(json!({"command": "echo long"})),
        &output,
        false,
        /*expanded*/ false,
        /*width*/ 24,
    ));

    assert!(
        out.contains("… +"),
        "wrapped long output should collapse to an ellipsis: {out}"
    );
}
