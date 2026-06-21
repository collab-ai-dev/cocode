use super::*;

fn snap(id: &str, status: &str) -> AsyncAgentSnapshot {
    AsyncAgentSnapshot {
        task_id: id.to_string(),
        status: status.to_string(),
        description: "Generate a report".to_string(),
        delta_summary: Some("Halfway through".to_string()),
        output_file_path: format!("/tmp/{id}.out"),
    }
}

#[test]
fn empty_snapshot_produces_no_attachments() {
    assert!(create_async_agent_attachments(&[]).is_empty());
}

#[test]
fn each_snapshot_produces_one_task_status_attachment() {
    let snaps = vec![snap("ta-1", "running"), snap("ta-2", "completed")];
    let atts = create_async_agent_attachments(&snaps);
    assert_eq!(atts.len(), 2);
    for a in &atts {
        assert_eq!(a.kind, coco_types::AttachmentKind::TaskStatus);
    }
}

#[test]
fn rendered_text_includes_id_status_path_and_is_sr_wrapped() {
    let atts = create_async_agent_attachments(&[snap("ta-xyz", "running")]);
    let LlmMessage::User { content, .. } = atts[0].as_api_message().unwrap() else {
        panic!("expected User LlmMessage");
    };
    let text = match &content[0] {
        coco_llm_types::UserContentPart::Text(t) => &t.text,
        _ => panic!("expected text part"),
    };
    assert!(
        text.starts_with("<system-reminder>\n"),
        "must be SR-wrapped"
    );
    assert!(text.contains("ta-xyz"), "must include task_id");
    assert!(text.contains("running"), "must include status");
    assert!(text.contains("/tmp/ta-xyz.out"), "must include output path");
    assert!(
        text.contains("Halfway through"),
        "must include delta summary"
    );
}

#[test]
fn missing_delta_and_path_omits_lines() {
    let s = AsyncAgentSnapshot {
        task_id: "ta-1".into(),
        status: "running".into(),
        description: "x".into(),
        delta_summary: None,
        output_file_path: String::new(),
    };
    let atts = create_async_agent_attachments(&[s]);
    let LlmMessage::User { content, .. } = atts[0].as_api_message().unwrap() else {
        panic!("expected User LlmMessage");
    };
    let text = match &content[0] {
        coco_llm_types::UserContentPart::Text(t) => &t.text,
        _ => panic!("expected text part"),
    };
    assert!(
        !text.contains("Summary:"),
        "no summary line when delta is None"
    );
    assert!(
        !text.contains("Output file:"),
        "no output line when path empty"
    );
}
