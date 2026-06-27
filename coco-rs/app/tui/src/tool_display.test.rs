use coco_types::PermissionDisplayInput;
use coco_types::ToolName;
use pretty_assertions::assert_eq;

use super::permission_display_input;
use super::render_tool_input_preview_spans;
use super::tool_input_preview;
use super::tool_input_semantic_preview;
use coco_tui_ui::display::SyntaxHighlighting;
use coco_tui_ui::style::UiStyles;
use coco_tui_ui::theme::Theme;

#[test]
fn test_permission_display_input_formats_glob_without_json() {
    let input = serde_json::json!({
        "path": "/Users/linyuzhi/codespace/myagent/codex",
        "pattern": "**/README.md"
    });

    assert_eq!(
        permission_display_input(ToolName::Glob.as_str(), &input),
        PermissionDisplayInput::Text(
            "path: /Users/linyuzhi/codespace/myagent/codex\npattern: **/README.md".into()
        )
    );
}

#[test]
fn test_tool_input_preview_formats_glob_as_single_line() {
    let input = serde_json::json!({
        "path": "/Users/linyuzhi/codespace/myagent/codex",
        "pattern": "**/README.md"
    });

    assert_eq!(
        tool_input_preview(ToolName::Glob.as_str(), &input),
        "**/README.md in /Users/linyuzhi/codespace/myagent/codex"
    );
}

#[test]
fn test_read_preview_path_only_has_no_range_suffix() {
    let input = serde_json::json!({"file_path": "/repo/README.md"});
    assert_eq!(
        tool_input_preview(ToolName::Read.as_str(), &input),
        "/repo/README.md"
    );
}

#[test]
fn test_read_preview_with_limit_shows_line_range() {
    let input = serde_json::json!({"file_path": "/repo/README.md", "limit": 10});
    assert_eq!(
        tool_input_preview(ToolName::Read.as_str(), &input),
        "/repo/README.md · lines 1-10"
    );
}

#[test]
fn test_read_preview_with_offset_and_limit_shows_line_range() {
    let input = serde_json::json!({"file_path": "/repo/README.md", "offset": 5, "limit": 10});
    assert_eq!(
        tool_input_preview(ToolName::Read.as_str(), &input),
        "/repo/README.md · lines 5-14"
    );
}

#[test]
fn test_read_preview_with_offset_only_shows_from_line() {
    let input = serde_json::json!({"file_path": "/repo/README.md", "offset": 5});
    assert_eq!(
        tool_input_preview(ToolName::Read.as_str(), &input),
        "/repo/README.md · from line 5"
    );
}

#[test]
fn test_shell_permission_display_input_keeps_command_variant() {
    let input = serde_json::json!({"command": "ls -la"});

    assert_eq!(
        permission_display_input(ToolName::Bash.as_str(), &input),
        PermissionDisplayInput::Command("ls -la".into())
    );
}

#[test]
fn test_bash_semantic_preview_is_shell_command() {
    let input = serde_json::json!({"command": "git status --short"});

    let preview = tool_input_semantic_preview(ToolName::Bash.as_str(), &input);

    assert!(matches!(
        preview,
        super::ToolInputPreview::ShellCommand { command, syntax }
            if command == "git status --short" && syntax == "bash"
    ));
}

#[test]
fn test_disabled_syntax_highlighting_renders_plain_command_preview() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let preview = super::ToolInputPreview::ShellCommand {
        command: "echo hello".into(),
        syntax: "bash".into(),
    };

    let spans = render_tool_input_preview_spans(&preview, styles, SyntaxHighlighting::Disabled, 80);

    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].content.as_ref(), "echo hello");
}

#[test]
fn test_styled_preview_truncation_respects_display_width() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let preview = super::ToolInputPreview::Plain("echo 界🙂abcdef".into());

    let spans = render_tool_input_preview_spans(&preview, styles, SyntaxHighlighting::Disabled, 10);
    let text = spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert_eq!(unicode_width::UnicodeWidthStr::width(text.as_str()), 10);
    assert!(text.ends_with('…'));
}
