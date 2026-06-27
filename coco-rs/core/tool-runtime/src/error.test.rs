use super::*;
use coco_types::ApplyPatchPreview;
use coco_types::ToolDisplayData;

#[test]
fn tool_error_execution_failed_constructors_set_display_data() {
    let plain = ToolError::execution_failed("plain failure");
    let ToolError::ExecutionFailed {
        message,
        display_data,
        source,
    } = plain
    else {
        panic!("expected execution failure");
    };
    assert_eq!(message, "plain failure");
    assert!(display_data.is_none());
    assert!(source.is_none());

    let with_display = ToolError::execution_failed_with_display_data(
        "preview failure",
        ToolDisplayData::ApplyPatchPreview(ApplyPatchPreview { rows: vec![] }),
    );
    let ToolError::ExecutionFailed {
        message,
        display_data,
        source,
    } = with_display
    else {
        panic!("expected execution failure");
    };
    assert_eq!(message, "preview failure");
    assert!(matches!(
        display_data,
        Some(ToolDisplayData::ApplyPatchPreview(_))
    ));
    assert!(source.is_none());
}

#[test]
fn format_tool_error_truncates_utf8_without_panicking() {
    let repeated = "好".repeat(4_000);
    let error = ToolError::execution_failed(format!("prefix-{repeated}-suffix"));

    let formatted = format_tool_error(&error);

    assert!(formatted.contains("prefix-"));
    assert!(formatted.contains("-suffix"));
    assert!(formatted.contains("chars truncated"));
    assert!(formatted.is_char_boundary(formatted.len()));
}
