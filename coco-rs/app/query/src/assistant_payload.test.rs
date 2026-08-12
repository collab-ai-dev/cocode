use super::*;

#[tokio::test]
async fn oversized_file_is_externalized_to_a_typed_reference() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let payload = "YWFh".repeat(MAX_INLINE_ASSISTANT_MEDIA_BASE64_BYTES / 4 + 1);
    let parts = vec![AssistantContentPart::File(
        coco_llm_types::FilePart::from_base64(payload, "image/png"),
    )];
    let uuid = Uuid::new_v4();

    let externalized =
        externalize_assistant_payloads(parts, Some(tempdir.path().to_path_buf()), uuid)
            .await
            .expect("externalize");

    let AssistantContentPart::File(file) = &externalized[0] else {
        panic!("expected file");
    };
    let reference = file.data.as_reference().expect("typed reference");
    let relative = reference.get("coco").expect("coco reference");
    assert!(tempdir.path().join(relative).is_file());
    assert!(serde_json::to_vec(&externalized).unwrap().len() < 2_000);
}

#[tokio::test]
async fn oversized_opaque_metadata_is_rejected_without_truncation() {
    let mut metadata = coco_llm_types::ProviderMetadata::default();
    metadata.0.insert(
        "provider".into(),
        serde_json::json!({"opaque": "x".repeat(MAX_OPAQUE_STRUCTURED_PART_BYTES)}),
    );
    let parts = vec![AssistantContentPart::Custom(
        coco_llm_types::CustomPart::new("opaque").with_provider_metadata(metadata),
    )];

    let error = externalize_assistant_payloads(parts, None, Uuid::new_v4())
        .await
        .expect_err("opaque state must be bounded");
    assert!(error.to_string().contains("safety limit"));
}

#[tokio::test]
async fn oversized_file_metadata_is_rejected_after_media_externalization() {
    let mut metadata = coco_llm_types::ProviderMetadata::default();
    metadata.0.insert(
        "provider".into(),
        serde_json::json!({"opaque": "x".repeat(MAX_OPAQUE_STRUCTURED_PART_BYTES)}),
    );
    let mut file = coco_llm_types::FilePart::from_base64(
        "YWFh".repeat(MAX_INLINE_ASSISTANT_MEDIA_BASE64_BYTES / 4 + 1),
        "image/png",
    );
    file.provider_metadata = Some(metadata);

    let error = externalize_assistant_payloads(
        vec![AssistantContentPart::File(file)],
        None,
        Uuid::new_v4(),
    )
    .await
    .expect_err("file metadata must be bounded independently of media");

    assert!(error.to_string().contains("safety limit"));
}

#[tokio::test]
async fn oversized_provider_tool_result_is_rejected_by_the_turn_limit() {
    let result = coco_llm_types::ToolResultPart::new(
        "provider-call",
        "provider-tool",
        coco_llm_types::ToolResultContent::text("x".repeat(MAX_ASSISTANT_TURN_BYTES)),
    );

    let error = externalize_assistant_payloads(
        vec![AssistantContentPart::ToolResult(result)],
        None,
        Uuid::new_v4(),
    )
    .await
    .expect_err("provider result must not bypass the total turn bound");

    assert!(error.to_string().contains("assistant turn"));
    assert!(error.to_string().contains("safety limit"));
}
