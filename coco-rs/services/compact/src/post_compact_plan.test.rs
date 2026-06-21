use super::*;
use std::path::PathBuf;

#[test]
fn returns_none_when_plan_content_absent() {
    assert!(create_plan_attachment_if_needed(&PathBuf::from("/tmp/plan.md"), None).is_none());
}

#[test]
fn returns_none_when_plan_content_empty() {
    assert!(create_plan_attachment_if_needed(&PathBuf::from("/tmp/plan.md"), Some("")).is_none());
    assert!(
        create_plan_attachment_if_needed(&PathBuf::from("/tmp/plan.md"), Some("   \n\n  "))
            .is_none()
    );
}

#[test]
fn emits_attachment_with_verbatim_template() {
    let path = PathBuf::from("/home/user/.coco/plans/deploy.md");
    let content = "1. Build the binary\n2. Push to registry";
    let att = create_plan_attachment_if_needed(&path, Some(content)).expect("emits");
    assert_eq!(att.kind, coco_types::AttachmentKind::PlanFileReference);
    let llm = att.as_api_message().expect("api body");
    let LlmMessage::User { content: parts, .. } = llm else {
        panic!("expected user message");
    };
    assert_eq!(parts.len(), 1);
    let coco_llm_types::UserContentPart::Text(tp) = &parts[0] else {
        panic!("expected text part");
    };
    let text = tp.text.as_str();
    assert!(text.starts_with("<system-reminder>"));
    assert!(text.ends_with("</system-reminder>"));
    // All expected template markers are present.
    assert!(
        text.contains("A plan file exists from plan mode at: /home/user/.coco/plans/deploy.md")
    );
    assert!(text.contains("Plan contents:\n\n1. Build the binary\n2. Push to registry"));
    assert!(text.contains(
        "If this plan is relevant to the current work and not already complete, continue working on it."
    ));
    // No `<plan>` XML wrapping (that was a coco-rs-only drift).
    assert!(!text.contains("<plan>"));
    assert!(!text.contains("</plan>"));
}

#[test]
fn owned_convenience_works() {
    let att =
        create_plan_attachment_from_owned(PathBuf::from("/p.md"), Some("content".to_string()));
    assert!(att.is_some());
}
