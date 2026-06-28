use std::io::Write;

use crate::ReadEvidence;
use crate::file_mtime_ms;

use super::*;

async fn detect_apply(state: &mut FileReadState) -> Vec<Attachment> {
    let candidates = changed_file_candidates(state);
    let mut observations = Vec::new();
    for candidate in candidates {
        let metadata = match tokio::fs::metadata(&candidate.path).await {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                observations.push(deleted_changed_file_observation(candidate.path));
                continue;
            }
            Err(_) => continue,
        };
        let mtime_ms = metadata
            .modified()
            .ok()
            .and_then(|mtime| mtime.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        if mtime_ms <= candidate.cached_mtime_ms {
            continue;
        }
        let loaded = if candidate.path.extension().and_then(|ext| ext.to_str()) == Some("png") {
            ChangedFileLoadedContent::Image { mtime_ms }
        } else {
            ChangedFileLoadedContent::Text {
                content: tokio::fs::read_to_string(&candidate.path).await.unwrap(),
                mtime_ms,
            }
        };
        observations.push(changed_file_observation_from_loaded(candidate, loaded));
    }
    let attachments = attachments_from_changed_file_observations(&observations);
    apply_changed_file_observations(state, &observations);
    attachments
}

#[tokio::test]
async fn test_no_changes() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("stable.txt");
    {
        let mut f = std::fs::File::create(&file).unwrap();
        f.write_all(b"stable content").unwrap();
    }
    let mtime = file_mtime_ms(&file).await.unwrap();

    let mut state = FileReadState::new();
    state.set(
        file.clone(),
        FileReadEntry::full_real("stable content".to_string(), mtime),
    );

    let changed = detect_apply(&mut state).await;
    assert!(changed.is_empty());
}

#[tokio::test]
async fn test_detects_changed_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("changing.txt");
    {
        let mut f = std::fs::File::create(&file).unwrap();
        f.write_all(b"original").unwrap();
    }
    let mtime = file_mtime_ms(&file).await.unwrap();

    let mut state = FileReadState::new();
    state.set(
        file.clone(),
        FileReadEntry::full_real("original".to_string(), mtime),
    );

    // Modify file after a small delay to ensure mtime changes
    std::thread::sleep(std::time::Duration::from_millis(50));
    {
        let mut f = std::fs::File::create(&file).unwrap();
        f.write_all(b"modified content").unwrap();
    }

    let changed = detect_apply(&mut state).await;
    assert_eq!(changed.len(), 1);
    match &changed[0] {
        Attachment::EditedTextFile(f) => {
            assert!(f.snippet.contains("modified content"));
            assert!(!f.snippet.contains("original"));
        }
        other => panic!("Expected EditedTextFile, got {other:?}"),
    }

    // State should be updated
    let entry = state.peek(&file).unwrap();
    assert_eq!(entry.content, "modified content");
    assert_eq!(entry.evidence, ReadEvidence::ObservedForDiff);
}

#[tokio::test]
async fn test_skips_partial_reads() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("partial.txt");
    {
        let mut f = std::fs::File::create(&file).unwrap();
        f.write_all(b"content").unwrap();
    }

    let mut state = FileReadState::new();
    state.set(
        file.clone(),
        FileReadEntry::line_real("partial".to_string(), 0, Some(5), 10),
    );

    // Even though mtime is stale, partial reads should be skipped
    let changed = detect_apply(&mut state).await;
    assert!(changed.is_empty());
}

#[tokio::test]
async fn test_touched_but_unchanged_refreshes_cache_without_attachment() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("touched.txt");
    {
        let mut f = std::fs::File::create(&file).unwrap();
        f.write_all(b"same content").unwrap();
    }
    let old_mtime = file_mtime_ms(&file).await.unwrap();

    let mut state = FileReadState::new();
    state.set(
        file.clone(),
        FileReadEntry::full_real("same content".to_string(), old_mtime),
    );

    std::thread::sleep(std::time::Duration::from_millis(50));
    {
        let mut f = std::fs::File::create(&file).unwrap();
        f.write_all(b"same content").unwrap();
    }

    let changed = detect_apply(&mut state).await;
    assert!(changed.is_empty());
    let refreshed = state.peek(&file).unwrap();
    assert!(
        refreshed.mtime_ms > old_mtime,
        "mtime should refresh even when no snippet is emitted"
    );
    assert_eq!(refreshed.evidence, ReadEvidence::RealFileView);
}

#[tokio::test]
async fn test_touched_but_unchanged_preserves_read_tool_origin() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("read-origin.txt");
    {
        let mut f = std::fs::File::create(&file).unwrap();
        f.write_all(b"same content").unwrap();
    }
    let old_mtime = file_mtime_ms(&file).await.unwrap();

    let mut state = FileReadState::new();
    state.set_from_read(
        file.clone(),
        FileReadEntry::full_real("same content".to_string(), old_mtime),
        None,
        None,
    );

    std::thread::sleep(std::time::Duration::from_millis(50));
    {
        let mut f = std::fs::File::create(&file).unwrap();
        f.write_all(b"same content").unwrap();
    }

    let changed = detect_apply(&mut state).await;
    assert!(changed.is_empty());
    assert!(state.is_from_read_tool(&file));
    let refreshed = state.peek(&file).unwrap();
    assert_eq!(refreshed.evidence, ReadEvidence::RealFileView);
}

#[tokio::test]
async fn test_detects_changed_image_file_as_silent_marker() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("screen.png");
    {
        let mut f = std::fs::File::create(&file).unwrap();
        f.write_all(b"old image bytes").unwrap();
    }
    let old_mtime = file_mtime_ms(&file).await.unwrap();

    let mut state = FileReadState::new();
    state.set(
        file.clone(),
        FileReadEntry::full_real(String::new(), old_mtime),
    );

    std::thread::sleep(std::time::Duration::from_millis(50));
    {
        let mut f = std::fs::File::create(&file).unwrap();
        f.write_all(b"new image bytes").unwrap();
    }

    let changed = detect_apply(&mut state).await;
    assert_eq!(changed.len(), 1);
    match &changed[0] {
        Attachment::EditedImageFile(f) => assert_eq!(f.filename, file.to_string_lossy()),
        other => panic!("Expected EditedImageFile, got {other:?}"),
    }

    let refreshed = state.peek(&file).unwrap();
    assert_eq!(refreshed.content, "");
    assert_eq!(refreshed.evidence, ReadEvidence::ObservedForDiff);
    assert!(refreshed.mtime_ms > old_mtime);
}

#[tokio::test]
async fn test_deleted_file_evicts_cache_entry() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("deleted.txt");
    {
        let mut f = std::fs::File::create(&file).unwrap();
        f.write_all(b"gone soon").unwrap();
    }
    let old_mtime = file_mtime_ms(&file).await.unwrap();

    let mut state = FileReadState::new();
    state.set(
        file.clone(),
        FileReadEntry::full_real("gone soon".to_string(), old_mtime),
    );

    std::fs::remove_file(&file).unwrap();

    let changed = detect_apply(&mut state).await;
    assert!(changed.is_empty());
    assert!(state.peek(&file).is_none());
}
