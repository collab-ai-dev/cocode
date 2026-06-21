use super::*;

fn temp_session_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("coco-disk-{}", uuid::Uuid::new_v4().simple()))
}

#[tokio::test]
async fn append_then_read_returns_full_content() {
    let outputs = DiskOutputs::new(temp_session_dir());
    let dto = outputs.get_or_create("t1").await;
    dto.append("hello ");
    dto.append("world");
    dto.flush().await.unwrap();
    let (content, offset) = dto.read_delta(0, 1024).await.unwrap();
    assert_eq!(content, "hello world");
    assert_eq!(offset, 11);
}

#[tokio::test]
async fn read_delta_from_nonzero_offset_returns_tail() {
    let outputs = DiskOutputs::new(temp_session_dir());
    let dto = outputs.get_or_create("t2").await;
    dto.append("abcdefghij");
    dto.flush().await.unwrap();
    let (content, offset) = dto.read_delta(5, 1024).await.unwrap();
    assert_eq!(content, "fghij");
    assert_eq!(offset, 10);
}

#[tokio::test]
async fn read_before_any_write_returns_empty() {
    let outputs = DiskOutputs::new(temp_session_dir());
    let dto = outputs.get_or_create("t3").await;
    let (content, offset) = dto.read_delta(0, 1024).await.unwrap();
    assert!(content.is_empty());
    assert_eq!(offset, 0);
}

#[tokio::test]
async fn evict_keeps_file_on_disk() {
    let outputs = DiskOutputs::new(temp_session_dir());
    let dto = outputs.get_or_create("t4").await;
    let path = dto.path().to_path_buf();
    dto.append("data");
    dto.flush().await.unwrap();
    drop(dto);
    outputs.evict("t4").await;
    // Brief settle for the drain task to release the file.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(path.exists(), "evict must NOT delete the file");
}

#[tokio::test]
async fn cleanup_removes_file() {
    let outputs = DiskOutputs::new(temp_session_dir());
    let dto = outputs.get_or_create("t5").await;
    let path = dto.path().to_path_buf();
    dto.append("data");
    dto.flush().await.unwrap();
    drop(dto);
    outputs.cleanup("t5").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(!path.exists(), "cleanup must unlink the file");
}

#[tokio::test]
async fn output_path_uses_session_dir_and_task_id() {
    let dir = temp_session_dir();
    let outputs = DiskOutputs::new(dir.clone());
    let path = outputs.output_path("agent-7af2");
    assert_eq!(path, dir.join("agent-7af2.output"));
}

#[tokio::test]
async fn read_tail_returns_full_content_when_under_cap() {
    let outputs = DiskOutputs::new(temp_session_dir());
    let dto = outputs.get_or_create("tt1").await;
    dto.append("the whole story");
    dto.flush().await.unwrap();
    let tail = dto.read_tail(1024).await.unwrap();
    assert_eq!(tail, "the whole story");
    assert!(
        !tail.contains("omitted"),
        "no omitted-bytes header when under cap"
    );
}

#[tokio::test]
async fn read_tail_prepends_omitted_header_when_over_cap() {
    let outputs = DiskOutputs::new(temp_session_dir());
    let dto = outputs.get_or_create("tt2").await;
    // Write 2 KB of "x" then a tail marker. Cap the read at 100
    // bytes — the head should be omitted with a header.
    dto.append(&"x".repeat(2048));
    dto.append("TAIL_MARKER_HERE");
    dto.flush().await.unwrap();
    let tail = dto.read_tail(100).await.unwrap();
    assert!(
        tail.starts_with("["),
        "must prepend an omitted-bytes header; got {:?}",
        &tail[..tail.len().min(40)]
    );
    assert!(tail.contains("KB of earlier output omitted"));
    assert!(
        tail.contains("TAIL_MARKER_HERE"),
        "tail content must be preserved"
    );
}

#[tokio::test]
async fn read_tail_empty_when_no_writes() {
    let outputs = DiskOutputs::new(temp_session_dir());
    let dto = outputs.get_or_create("tt3").await;
    let tail = dto.read_tail(1024).await.unwrap();
    assert!(tail.is_empty());
}

#[tokio::test]
async fn get_or_create_is_idempotent() {
    let outputs = DiskOutputs::new(temp_session_dir());
    let a = outputs.get_or_create("t6").await;
    let b = outputs.get_or_create("t6").await;
    a.append("first");
    a.flush().await.unwrap();
    let (content, _) = b.read_delta(0, 1024).await.unwrap();
    assert_eq!(content, "first", "both handles point at the same file");
}
