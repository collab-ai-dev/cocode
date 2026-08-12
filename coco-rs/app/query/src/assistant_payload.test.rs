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
async fn oversized_file_without_artifact_store_is_rejected() {
    let payload = "YWFh".repeat(MAX_INLINE_ASSISTANT_MEDIA_BASE64_BYTES / 4 + 1);
    let parts = vec![AssistantContentPart::File(
        coco_llm_types::FilePart::from_base64(payload, "image/png"),
    )];

    let error = externalize_assistant_payloads(parts, None, Uuid::new_v4())
        .await
        .expect_err("large media requires durable storage");

    assert!(error.to_string().contains("requires an artifact store"));
}

#[tokio::test]
async fn oversized_file_metadata_is_rejected_after_media_externalization() {
    let tempdir = tempfile::tempdir().expect("tempdir");
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
        Some(tempdir.path().to_path_buf()),
        Uuid::new_v4(),
    )
    .await
    .expect_err("file metadata must be bounded independently of media");

    assert!(error.to_string().contains("safety limit"));
    assert_eq!(
        std::fs::read_dir(tempdir.path().join("assistant-media"))
            .expect("artifact directory")
            .count(),
        0,
        "failed validation must roll back newly-created artifacts"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn artifact_write_rejects_a_symlinked_media_directory() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let root = tempdir.path().join("store");
    let outside = tempdir.path().join("outside");
    std::fs::create_dir_all(&root).expect("artifact root");
    std::fs::create_dir_all(&outside).expect("outside dir");
    std::os::unix::fs::symlink(&outside, root.join("assistant-media")).expect("media symlink");
    let payload = "YWFh".repeat(MAX_INLINE_ASSISTANT_MEDIA_BASE64_BYTES / 4 + 1);
    let parts = vec![AssistantContentPart::File(
        coco_llm_types::FilePart::from_base64(payload, "image/png"),
    )];

    let error = externalize_assistant_payloads(parts, Some(root), Uuid::new_v4())
        .await
        .expect_err("artifact writes must stay inside the configured store");

    assert!(error.to_string().contains("symbolic link"));
    assert_eq!(
        std::fs::read_dir(outside).expect("outside listing").count(),
        0
    );
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

#[tokio::test]
async fn provider_tool_media_is_externalized_and_rehydrated_without_losing_association() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let payload = "YWFh".repeat(MAX_INLINE_ASSISTANT_MEDIA_BASE64_BYTES / 4 + 1);
    let result = coco_llm_types::ToolResultPart::new(
        "image-call",
        "image_generation",
        coco_llm_types::ToolResultContent::content_parts(vec![
            coco_llm_types::ToolResultContentPart::file_data(&payload, "image/png"),
        ]),
    );
    let externalized = externalize_assistant_payloads(
        vec![AssistantContentPart::ToolResult(result)],
        Some(tempdir.path().to_path_buf()),
        Uuid::new_v4(),
    )
    .await
    .expect("externalize tool result");

    let AssistantContentPart::ToolResult(result) = &externalized[0] else {
        panic!("expected tool result");
    };
    let coco_llm_types::ToolResultContent::Content { value, .. } = &result.output else {
        panic!("expected content output");
    };
    assert!(matches!(
        &value[0],
        coco_llm_types::ToolResultContentPart::FileReference { .. }
    ));

    let prompt = vec![coco_llm_types::LlmMessage::Assistant {
        content: externalized,
        provider_options: None,
    }];
    let rehydrated = rehydrate_assistant_payloads(prompt, Some(tempdir.path().to_path_buf()))
        .await
        .expect("rehydrate tool result");
    let coco_llm_types::LlmMessage::Assistant { content, .. } = &rehydrated[0] else {
        panic!("expected assistant message");
    };
    let AssistantContentPart::ToolResult(result) = &content[0] else {
        panic!("expected tool result");
    };
    let coco_llm_types::ToolResultContent::Content { value, .. } = &result.output else {
        panic!("expected content output");
    };
    assert!(matches!(
        &value[0],
        coco_llm_types::ToolResultContentPart::FileData { data, media_type, .. }
            if data == &payload && media_type == "image/png"
    ));
}

#[tokio::test]
async fn rehydration_rejects_artifact_path_traversal() {
    let result = coco_llm_types::ToolResultPart::new(
        "image-call",
        "image_generation",
        coco_llm_types::ToolResultContent::content_parts(vec![
            coco_llm_types::ToolResultContentPart::file_reference(HashMap::from([
                (COCO_ARTIFACT_REFERENCE.to_string(), "../secret".to_string()),
                (
                    COCO_ARTIFACT_MEDIA_TYPE.to_string(),
                    "image/png".to_string(),
                ),
            ])),
        ]),
    );
    let prompt = vec![coco_llm_types::LlmMessage::Assistant {
        content: vec![AssistantContentPart::ToolResult(result)],
        provider_options: None,
    }];

    let error = rehydrate_assistant_payloads(prompt, Some(PathBuf::from("/tmp")))
        .await
        .expect_err("path traversal must be rejected");
    assert!(
        error
            .to_string()
            .contains("invalid generated media reference")
    );
}

#[test]
fn artifact_read_is_bounded_before_allocating_the_whole_file() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let relative = Path::new("assistant-media").join("large.bin");
    std::fs::create_dir_all(tempdir.path().join("assistant-media")).expect("artifact dir");
    std::fs::write(tempdir.path().join(&relative), [0_u8; 16]).expect("artifact");
    let mut budget = RehydrationBudget {
        encoded_bytes: 0,
        limit: 8,
    };

    let error = read_artifact(
        tempdir.path(),
        relative.to_str().expect("utf-8 path"),
        &mut budget,
    )
    .expect_err("artifact must respect rehydration budget");

    assert!(error.to_string().contains("8-byte safety limit"));
}
