use super::*;
use coco_tool_runtime::ToolUseContext;
use serde_json::json;

#[test]
fn passthrough_exposes_original_as_exec_and_stamps_nothing() {
    let cmd = ResolvedCommand::Passthrough("git status".to_string());
    assert_eq!(cmd.exec(), "git status");
    assert_eq!(cmd.original(), "git status");
    assert!(!cmd.was_rewritten());

    let mut obj = json!({ "command": "git status" });
    cmd.annotate_envelope(&mut obj);
    assert!(obj.get("rtk").is_none());
    assert!(obj.get("rtkCommand").is_none());
}

#[test]
fn rewritten_keeps_original_and_stamps_provenance() {
    let cmd = ResolvedCommand::Rewritten {
        original: "git status".to_string(),
        exec: "rtk git status".to_string(),
    };
    assert_eq!(cmd.exec(), "rtk git status");
    assert_eq!(cmd.original(), "git status");
    assert!(cmd.was_rewritten());

    let mut obj = json!({ "command": "git status" });
    cmd.annotate_envelope(&mut obj);
    // The envelope keeps the model-issued command; provenance is additive.
    assert_eq!(obj["command"], "git status");
    assert_eq!(obj["rtk"], "external");
    assert_eq!(obj["rtkCommand"], "rtk git status");
}

#[tokio::test]
async fn resolve_passthrough_when_no_rewriter() {
    // `ToolUseContext::test_default()` leaves `rtk = None` (feature off), so a
    // command resolves to a passthrough with no rtk envelope fields.
    let ctx = ToolUseContext::test_default();
    let resolved = resolve_rtk_command(
        "git status",
        /*run_in_background*/ false,
        /*sandbox_active*/ false,
        &ctx,
    )
    .await;
    assert!(!resolved.was_rewritten());
    assert_eq!(resolved.exec(), "git status");
    assert_eq!(resolved.original(), "git status");
}
