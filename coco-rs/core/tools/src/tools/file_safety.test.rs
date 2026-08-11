use super::*;

#[test]
fn distinguishes_regular_files_directories_and_missing_paths() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("file.txt");
    std::fs::write(&file, "content").unwrap();

    assert_eq!(inspect_file_target(&file).unwrap(), FileTargetKind::Regular);
    assert_eq!(
        inspect_file_target(temp.path()).unwrap(),
        FileTargetKind::Directory
    );
    assert_eq!(
        inspect_file_target(&temp.path().join("missing"))
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
}

#[cfg(unix)]
#[test]
fn detects_arbitrary_fifos_without_opening_them() {
    let temp = tempfile::tempdir().unwrap();
    let fifo = temp.path().join("custom-pipe");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(status.success());

    assert_eq!(inspect_file_target(&fifo).unwrap(), FileTargetKind::Fifo);
}

#[tokio::test]
async fn missing_path_reports_unicode_equivalent_and_nearby_names() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("release notes.md"), "content").unwrap();
    std::fs::write(temp.path().join("configuration.toml"), "content").unwrap();

    let mut ctx = coco_tool_runtime::ToolUseContext::test_default();
    ctx.cwd_override = Some(temp.path().to_path_buf());
    let unicode_message =
        missing_path_message(&temp.path().join("release\u{202f}notes.md"), &ctx).await;
    assert!(unicode_message.contains("Unicode-equivalent path exists"));
    assert!(unicode_message.contains("release notes.md"));

    let typo_message = missing_path_message(&temp.path().join("configuraton.toml"), &ctx).await;
    assert!(typo_message.contains("Did you mean"));
    assert!(typo_message.contains("configuration.toml"));
}

#[tokio::test]
async fn missing_bare_relative_path_scans_the_current_directory() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("configuration.toml"), "content").unwrap();
    let mut ctx = coco_tool_runtime::ToolUseContext::test_default();
    ctx.cwd_override = Some(temp.path().to_path_buf());

    let message = missing_path_message(Path::new("configuraton.toml"), &ctx).await;

    assert!(message.contains("configuration.toml"));
}

#[tokio::test]
async fn oversized_directories_do_not_return_nondeterministic_suggestions() {
    let temp = tempfile::tempdir().unwrap();
    for index in 0..=MAX_SUGGESTION_ENTRIES {
        std::fs::write(temp.path().join(format!("candidate-{index:03}.txt")), "x").unwrap();
    }
    let mut ctx = coco_tool_runtime::ToolUseContext::test_default();
    ctx.cwd_override = Some(temp.path().to_path_buf());

    let message = missing_path_message(&temp.path().join("candidate-001.tx"), &ctx).await;

    assert!(!message.contains("Did you mean"));
}

#[tokio::test]
async fn missing_path_does_not_leak_siblings_without_parent_read_permission() {
    let cwd = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("sensitive-name.txt"), "content").unwrap();
    let mut ctx = coco_tool_runtime::ToolUseContext::test_default();
    ctx.cwd_override = Some(cwd.path().to_path_buf());

    let message = missing_path_message(&outside.path().join("sensitive-nam.txt"), &ctx).await;

    assert!(!message.contains("sensitive-name.txt"));
    assert!(!message.contains("Did you mean"));
}

#[tokio::test]
async fn missing_path_suggestions_respect_file_read_ignore_patterns() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("private-token.txt"), "content").unwrap();
    let mut ctx = coco_tool_runtime::ToolUseContext::test_default();
    ctx.cwd_override = Some(temp.path().to_path_buf());
    ctx.tool_config.file_read_ignore_patterns = vec!["private-token.txt".to_string()];

    let message = missing_path_message(&temp.path().join("private-toke.txt"), &ctx).await;

    assert!(!message.contains("private-token.txt"));
    assert!(!message.contains("Did you mean"));
}

#[test]
fn atomic_commit_verifies_exact_on_disk_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("file.txt");

    let verified = commit_file(&file, b"expected").unwrap();

    assert_eq!(std::fs::read(&file).unwrap(), b"expected");
    assert_eq!(
        serde_json::to_value(verified).unwrap(),
        serde_json::json!(true)
    );
    assert!(serde_json::from_value::<VerifiedWrite>(serde_json::json!(false)).is_err());
}
