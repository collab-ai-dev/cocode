use std::io::Write;

use super::*;
use crate::ReadEvidence;

fn make_mention(text: &str, mention_type: MentionType) -> Mention {
    Mention {
        text: text.to_string(),
        mention_type,
        start: 0,
        end: text.len(),
        line_start: None,
        line_end: None,
    }
}

#[tokio::test]
async fn test_resolve_file_mention() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.rs");
    {
        let mut f = std::fs::File::create(&file_path).unwrap();
        f.write_all(b"fn main() {}\n").unwrap();
    }
    let mut state = FileReadState::new();
    let options = MentionResolveOptions {
        cwd: dir.path(),
        max_dir_entries: 100,
    };

    let mentions = vec![make_mention("test.rs", MentionType::FilePath)];
    let attachments = resolve_mentions(&mentions, &mut state, &options).await;

    assert_eq!(attachments.len(), 1);
    match &attachments[0] {
        Attachment::File(f) => {
            assert!(f.content.contains("fn main()"));
            assert_eq!(f.display_path, "test.rs");
        }
        other => panic!("Expected File attachment, got {other:?}"),
    }

    // FileReadState should now have the entry
    assert_eq!(state.len(), 1);
    let abs = std::fs::canonicalize(&file_path).unwrap();
    let entry = state.peek(&abs).expect("mention should populate state");
    assert_eq!(entry.range, FileReadRange::Full);
    assert_eq!(entry.evidence, ReadEvidence::RealFileView);
}

#[tokio::test]
async fn test_resolve_already_read_file() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("cached.rs");
    {
        let mut f = std::fs::File::create(&file_path).unwrap();
        f.write_all(b"cached content\n").unwrap();
    }

    let mut state = FileReadState::new();
    let options = MentionResolveOptions {
        cwd: dir.path(),
        max_dir_entries: 100,
    };

    // First resolve: should create File attachment
    let mentions = vec![make_mention("cached.rs", MentionType::FilePath)];
    let atts1 = resolve_mentions(&mentions, &mut state, &options).await;
    assert_eq!(atts1.len(), 1);
    assert!(matches!(&atts1[0], Attachment::File(_)));

    // Second resolve: should return AlreadyReadFile (file unchanged)
    let atts2 = resolve_mentions(&mentions, &mut state, &options).await;
    assert_eq!(atts2.len(), 1);
    assert!(
        matches!(&atts2[0], Attachment::AlreadyReadFile(_)),
        "Expected AlreadyReadFile, got {:?}",
        atts2[0]
    );
}

#[tokio::test]
async fn test_resolve_directory_mention() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("subdir");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("a.txt"), "a").unwrap();
    std::fs::write(sub.join("b.txt"), "b").unwrap();

    let mut state = FileReadState::new();
    let options = MentionResolveOptions {
        cwd: dir.path(),
        max_dir_entries: 100,
    };

    let mentions = vec![make_mention("subdir", MentionType::FilePath)];
    let attachments = resolve_mentions(&mentions, &mut state, &options).await;

    assert_eq!(attachments.len(), 1);
    match &attachments[0] {
        Attachment::Directory(d) => {
            assert!(d.content.contains("a.txt"));
            assert!(d.content.contains("b.txt"));
        }
        other => panic!("Expected Directory attachment, got {other:?}"),
    }
}

#[tokio::test]
async fn test_resolve_directory_mention_omits_trailing_slash() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(root.join("nested")).unwrap();
    std::fs::write(root.join("file.txt"), "x").unwrap();

    let mut state = FileReadState::new();
    let options = MentionResolveOptions {
        cwd: dir.path(),
        max_dir_entries: 100,
    };

    let mentions = vec![make_mention("root", MentionType::FilePath)];
    let attachments = resolve_mentions(&mentions, &mut state, &options).await;

    match &attachments[0] {
        // TS lists bare entry names — a subdirectory carries no trailing slash.
        Attachment::Directory(d) => {
            assert!(d.content.contains("nested"));
            assert!(!d.content.contains("nested/"));
            assert!(d.content.contains("file.txt"));
        }
        other => panic!("Expected Directory attachment, got {other:?}"),
    }
}

#[tokio::test]
async fn test_resolve_directory_mention_truncates_with_overflow_count() {
    let dir = tempfile::tempdir().unwrap();
    let big = dir.path().join("big");
    std::fs::create_dir(&big).unwrap();
    for i in 0..5 {
        std::fs::write(big.join(format!("f{i}.txt")), "x").unwrap();
    }

    let mut state = FileReadState::new();
    let options = MentionResolveOptions {
        cwd: dir.path(),
        max_dir_entries: 2,
    };

    let mentions = vec![make_mention("big", MentionType::FilePath)];
    let attachments = resolve_mentions(&mentions, &mut state, &options).await;

    match &attachments[0] {
        // TS mirror: exact overflow count (`… and N more entries`), not a
        // `1000+`-style cap marker. 2 listed names + 1 overflow line = 3.
        Attachment::Directory(d) => {
            assert!(d.content.contains("… and 3 more entries"));
            assert_eq!(d.content.lines().count(), 3);
        }
        other => panic!("Expected Directory attachment, got {other:?}"),
    }
}

#[tokio::test]
async fn test_resolve_agent_mention() {
    let mut state = FileReadState::new();
    let options = MentionResolveOptions::default();
    let mentions = vec![make_mention("agent-reviewer", MentionType::Agent)];
    let attachments = resolve_mentions(&mentions, &mut state, &options).await;

    assert_eq!(attachments.len(), 1);
    match &attachments[0] {
        Attachment::AgentMention(a) => assert_eq!(a.agent_type, "agent-reviewer"),
        other => panic!("Expected AgentMention, got {other:?}"),
    }
}

#[tokio::test]
async fn test_resolve_nonexistent_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = FileReadState::new();
    let options = MentionResolveOptions {
        cwd: dir.path(),
        max_dir_entries: 100,
    };
    let mentions = vec![make_mention("nonexistent.rs", MentionType::FilePath)];
    let attachments = resolve_mentions(&mentions, &mut state, &options).await;
    assert!(attachments.is_empty());
}

#[tokio::test]
async fn test_resolve_with_line_range() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("lines.rs");
    {
        let mut f = std::fs::File::create(&file_path).unwrap();
        for i in 1..=20 {
            writeln!(f, "line {i}").unwrap();
        }
    }
    let mut state = FileReadState::new();
    let options = MentionResolveOptions {
        cwd: dir.path(),
        max_dir_entries: 100,
    };

    let mentions = vec![Mention {
        text: "lines.rs".to_string(),
        mention_type: MentionType::FilePath,
        start: 0,
        end: 8,
        line_start: Some(5),
        line_end: Some(10),
    }];
    let attachments = resolve_mentions(&mentions, &mut state, &options).await;
    assert_eq!(attachments.len(), 1);
    match &attachments[0] {
        Attachment::File(f) => {
            assert_eq!(f.offset, Some(5));
        }
        other => panic!("Expected File attachment, got {other:?}"),
    }
    let abs = std::fs::canonicalize(&file_path).unwrap();
    let entry = state.peek(&abs).expect("mention should populate state");
    assert_eq!(
        entry.range,
        FileReadRange::Lines {
            offset: Some(5),
            limit: 6,
        }
    );
    assert_eq!(entry.evidence, ReadEvidence::InjectedPartialView);
}
