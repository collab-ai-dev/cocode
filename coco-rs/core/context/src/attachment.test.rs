use pretty_assertions::assert_eq;

use super::*;

// ---------------------------------------------------------------------------
// Token estimation tests
// ---------------------------------------------------------------------------

#[test]
fn test_file_attachment_estimated_tokens() {
    let att = Attachment::File(FileAttachment {
        filename: "/src/main.rs".to_owned(),
        content: "fn main() {\n    println!(\"hello\");\n}".to_owned(),
        truncated: false,
        display_path: "src/main.rs".to_owned(),
        offset: None,
        limit: None,
    });
    // ~36 chars / 4 ≈ 9 tokens + 20 overhead = 29
    let est = att.estimated_tokens();
    assert!(est > 0, "estimated tokens should be positive");
    assert!(
        est < 100,
        "small file should have low token count: got {est}"
    );
}

#[test]
fn test_already_read_file_is_cheap() {
    let att = Attachment::AlreadyReadFile(AlreadyReadFileAttachment {
        filename: "/src/lib.rs".to_owned(),
        display_path: "src/lib.rs".to_owned(),
    });
    assert_eq!(att.estimated_tokens(), 15);
}

#[test]
fn test_system_reminder_tokens_scale_with_content() {
    let short = Attachment::SystemReminder {
        attachment_type: "test".to_owned(),
        content: "hello".to_owned(),
    };
    let long = Attachment::SystemReminder {
        attachment_type: "test".to_owned(),
        content: "a".repeat(4000),
    };
    assert!(
        long.estimated_tokens() > short.estimated_tokens(),
        "longer content should cost more tokens"
    );
}

// ---------------------------------------------------------------------------
// Deduplication tests
// ---------------------------------------------------------------------------

#[test]
fn test_deduplicator_prevents_double_injection() {
    let mut dedup = AttachmentDeduplicator::new();

    let attachments = vec![
        Attachment::File(FileAttachment {
            filename: "/a.rs".to_owned(),
            content: "content".to_owned(),
            truncated: false,
            display_path: "a.rs".to_owned(),
            offset: None,
            limit: None,
        }),
        Attachment::File(FileAttachment {
            filename: "/a.rs".to_owned(),
            content: "same file again".to_owned(),
            truncated: false,
            display_path: "a.rs".to_owned(),
            offset: None,
            limit: None,
        }),
        Attachment::File(FileAttachment {
            filename: "/b.rs".to_owned(),
            content: "different file".to_owned(),
            truncated: false,
            display_path: "b.rs".to_owned(),
            offset: None,
            limit: None,
        }),
    ];

    let result = dedup.dedup_attachments(attachments);
    assert_eq!(result.len(), 2, "duplicate /a.rs should be removed");
    assert_eq!(dedup.loaded_count(), 2);
}

#[test]
fn test_deduplicator_relevant_memory_budget() {
    let mut dedup = AttachmentDeduplicator::new();

    // Simulate approaching budget
    dedup.add_relevant_memory_bytes(MAX_SESSION_MEMORY_BYTES - 100);
    assert!(!dedup.is_session_memory_exhausted());

    dedup.add_relevant_memory_bytes(200);
    assert!(dedup.is_session_memory_exhausted());
}

#[test]
fn test_dedup_relevant_memories_filters_loaded() {
    let mut dedup = AttachmentDeduplicator::new();
    dedup.mark_loaded("/memory/old.md");

    let attachment = RelevantMemoriesAttachment {
        memories: vec![
            RelevantMemoryEntry {
                path: "/memory/old.md".to_owned(),
                content: "stale".to_owned(),
                mtime_ms: 1000,
                header: None,
                limit: None,
            },
            RelevantMemoryEntry {
                path: "/memory/new.md".to_owned(),
                content: "fresh".to_owned(),
                mtime_ms: 2000,
                header: None,
                limit: None,
            },
        ],
    };

    let result = dedup.dedup_relevant_memories(attachment);
    let memories = result.expect("should have at least one memory");
    assert_eq!(memories.memories.len(), 1);
    assert_eq!(memories.memories[0].path, "/memory/new.md");
}

// ---------------------------------------------------------------------------
// Budget tests
// ---------------------------------------------------------------------------

#[test]
fn test_budget_admits_within_limit() {
    let mut budget = AttachmentBudget::new(100);

    let cheap = Attachment::AlreadyReadFile(AlreadyReadFileAttachment {
        filename: "/x.rs".to_owned(),
        display_path: "x.rs".to_owned(),
    });
    assert!(budget.try_admit(&cheap), "cheap attachment should fit");
    assert_eq!(budget.used_tokens(), 15);
    assert_eq!(budget.remaining(), 85);
}

#[test]
fn test_budget_rejects_over_limit() {
    let mut budget = AttachmentBudget::new(20);

    let expensive = Attachment::File(FileAttachment {
        filename: "/big.rs".to_owned(),
        content: "a".repeat(1000),
        truncated: false,
        display_path: "big.rs".to_owned(),
        offset: None,
        limit: None,
    });
    assert!(
        !budget.try_admit(&expensive),
        "expensive attachment should not fit in small budget"
    );
    assert_eq!(
        budget.used_tokens(),
        0,
        "failed admission should not charge"
    );
}

#[test]
fn test_budget_filter_preserves_order() {
    let mut budget = AttachmentBudget::new(50);

    let attachments = vec![
        Attachment::AlreadyReadFile(AlreadyReadFileAttachment {
            filename: "/a.rs".to_owned(),
            display_path: "a.rs".to_owned(),
        }),
        Attachment::AlreadyReadFile(AlreadyReadFileAttachment {
            filename: "/b.rs".to_owned(),
            display_path: "b.rs".to_owned(),
        }),
        Attachment::AlreadyReadFile(AlreadyReadFileAttachment {
            filename: "/c.rs".to_owned(),
            display_path: "c.rs".to_owned(),
        }),
        // This one pushes over: 15 * 4 = 60 > 50
        Attachment::AlreadyReadFile(AlreadyReadFileAttachment {
            filename: "/d.rs".to_owned(),
            display_path: "d.rs".to_owned(),
        }),
    ];

    let result = budget.filter_within_budget(attachments);
    assert_eq!(result.len(), 3, "only 3 of 4 should fit in budget of 50");
}

// ---------------------------------------------------------------------------
// File attachment generation tests
// ---------------------------------------------------------------------------

#[test]
fn test_generate_file_attachment_reads_file() {
    let dir = tempfile::tempdir().expect("tmpdir");
    let file_path = dir.path().join("test.rs");
    std::fs::write(&file_path, "fn main() {}\n").expect("write");

    let result = generate_file_attachment(&file_path, dir.path(), &FileReadOptions::default());
    let att = result.expect("should produce attachment");
    match att {
        Attachment::File(f) => {
            assert_eq!(f.content, "fn main() {}");
            assert!(!f.truncated);
            assert_eq!(f.display_path, "test.rs");
        }
        other => panic!("expected File attachment, got {other:?}"),
    }
}

#[test]
fn test_generate_file_attachment_with_offset_limit() {
    let dir = tempfile::tempdir().expect("tmpdir");
    let file_path = dir.path().join("lines.txt");
    std::fs::write(&file_path, "line1\nline2\nline3\nline4\nline5\n").expect("write");

    let result = generate_file_attachment(
        &file_path,
        dir.path(),
        &FileReadOptions {
            offset: Some(2),
            limit: Some(2),
            max_tokens: None,
        },
    );
    let att = result.expect("should produce attachment");
    match att {
        Attachment::File(f) => {
            assert_eq!(f.content, "line2\nline3");
            assert_eq!(f.offset, Some(2));
            assert_eq!(f.limit, Some(2));
        }
        other => panic!("expected File attachment, got {other:?}"),
    }
}

#[test]
fn test_generate_file_attachment_truncates_large() {
    let dir = tempfile::tempdir().expect("tmpdir");
    let file_path = dir.path().join("big.txt");
    // Write a file much larger than the token budget
    let large_content = "x".repeat(200_000);
    std::fs::write(&file_path, &large_content).expect("write");

    let result = generate_file_attachment(
        &file_path,
        dir.path(),
        &FileReadOptions {
            offset: None,
            limit: None,
            max_tokens: Some(100), // Very small budget
        },
    );
    let att = result.expect("should produce attachment");
    match att {
        Attachment::File(f) => {
            assert!(f.truncated, "should be truncated");
            assert!(
                f.content.len() < large_content.len(),
                "content should be shorter than original"
            );
        }
        other => panic!("expected File attachment, got {other:?}"),
    }
}

#[test]
fn test_generate_file_attachment_missing_file() {
    let dir = tempfile::tempdir().expect("tmpdir");
    let result = generate_file_attachment(
        &dir.path().join("nonexistent.rs"),
        dir.path(),
        &FileReadOptions::default(),
    );
    assert!(result.is_none(), "missing file should return None");
}

#[test]
fn test_image_path_detection() {
    let dir = tempfile::tempdir().expect("tmpdir");
    let img_path = dir.path().join("screenshot.png");
    std::fs::write(&img_path, b"fake png data").expect("write");

    let result = generate_file_attachment(&img_path, dir.path(), &FileReadOptions::default());
    match result {
        Some(Attachment::Image(img)) => {
            assert_eq!(img.media_type, "image/png");
            assert!(img.base64_data.is_some(), "base64 data should be populated");
            // "fake png data" → base64
            assert!(img.base64_data.unwrap().starts_with("ZmFr")); // "fak..." in base64
        }
        other => panic!("expected Image attachment, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Memory attachment tests
// ---------------------------------------------------------------------------

#[test]
fn test_load_memory_attachment() {
    let dir = tempfile::tempdir().expect("tmpdir");
    let mem_path = dir.path().join("CLAUDE.md");
    std::fs::write(&mem_path, "# Project Rules\n\nUse Rust.").expect("write");

    let result = load_memory_attachment(&mem_path, "project", dir.path());
    let att = result.expect("should produce attachment");
    match att {
        Attachment::NestedMemory(nm) => {
            assert_eq!(nm.memory_type, "project");
            assert!(nm.content.contains("Use Rust"));
            assert_eq!(nm.display_path, "CLAUDE.md");
        }
        other => panic!("expected NestedMemory attachment, got {other:?}"),
    }
}

#[test]
fn test_load_memory_attachment_truncates_by_bytes() {
    let dir = tempfile::tempdir().expect("tmpdir");
    let mem_path = dir.path().join("big_memory.md");
    // Write content larger than MAX_MEMORY_BYTES
    let big = "x".repeat(MAX_MEMORY_BYTES as usize + 1000);
    std::fs::write(&mem_path, &big).expect("write");

    let result = load_memory_attachment(&mem_path, "user", dir.path());
    let att = result.expect("should produce attachment");
    match att {
        Attachment::NestedMemory(nm) => {
            assert!(
                nm.content.contains("truncated"),
                "should contain truncation note"
            );
            // The truncation note adds ~100 chars, but the base content should
            // be <= MAX_MEMORY_BYTES + note length
        }
        other => panic!("expected NestedMemory attachment, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Helper tests
// ---------------------------------------------------------------------------

#[test]
fn test_slice_content_offset_limit() {
    let content = "a\nb\nc\nd\ne";
    let (sliced, total) = slice_content(content, Some(2), Some(2));
    assert_eq!(sliced, "b\nc");
    assert_eq!(total, 5);
}

#[test]
fn test_slice_content_no_offset() {
    let content = "line1\nline2\nline3";
    let (sliced, total) = slice_content(content, None, None);
    assert_eq!(sliced, "line1\nline2\nline3");
    assert_eq!(total, 3);
}

#[test]
fn test_truncate_to_char_boundary_ascii() {
    let s = "hello world";
    assert_eq!(truncate_to_char_boundary(s, 5), "hello");
}

#[test]
fn test_truncate_to_char_boundary_utf8() {
    // "Hello " = 6 bytes, each emoji = 4 bytes
    let s = "Hello 🌍🌎";
    // 10 bytes = "Hello " (6) + 🌍 (4) = exactly one emoji
    assert_eq!(truncate_to_char_boundary(s, 10), "Hello 🌍");
    // 9 bytes falls inside 🌍 → back up to byte 6 = "Hello "
    assert_eq!(truncate_to_char_boundary(s, 9), "Hello ");
    // 14 bytes = full string (6 + 4 + 4)
    assert_eq!(truncate_to_char_boundary(s, 14), "Hello 🌍🌎");
}

#[test]
fn test_batch_classification() {
    let file_att = Attachment::File(FileAttachment {
        filename: "/x.rs".to_owned(),
        content: "x".to_owned(),
        truncated: false,
        display_path: "x.rs".to_owned(),
        offset: None,
        limit: None,
    });
    assert_eq!(file_att.batch(), AttachmentBatch::UserInput);

    let skill_att = Attachment::SkillListing(SkillListingAttachment {
        content: "/commit — commit changes".to_owned(),
        skill_count: 1,
        is_initial: true,
    });
    assert_eq!(skill_att.batch(), AttachmentBatch::AllThread);

    let token_att = Attachment::TokenUsage(TokenUsageAttachment {
        used: 50_000,
        total: 200_000,
        remaining: 150_000,
    });
    assert_eq!(token_att.batch(), AttachmentBatch::MainThreadOnly);
}

#[test]
fn test_pdf_reference_attachment_serialization() {
    let att = Attachment::PdfReference(PdfReferenceAttachment {
        filename: "/docs/spec.pdf".to_owned(),
        page_count: 42,
        file_size: 4_200_000,
        display_path: "docs/spec.pdf".to_owned(),
    });

    let json = serde_json::to_string(&att).expect("serialize");
    assert!(json.contains("\"type\":\"pdf_reference\""));
    assert!(json.contains("\"page_count\":42"));

    let deserialized: Attachment = serde_json::from_str(&json).expect("deserialize");
    match deserialized {
        Attachment::PdfReference(pdf) => {
            assert_eq!(pdf.page_count, 42);
            assert_eq!(pdf.file_size, 4_200_000);
        }
        other => panic!("expected PdfReference, got {other:?}"),
    }
}

#[test]
fn test_deduplicator_reset() {
    let mut dedup = AttachmentDeduplicator::new();
    dedup.mark_loaded("/a.rs");
    dedup.add_relevant_memory_bytes(1000);

    assert_eq!(dedup.loaded_count(), 1);
    assert_eq!(dedup.relevant_memory_bytes(), 1000);

    dedup.reset();
    assert_eq!(dedup.loaded_count(), 0);
    assert_eq!(dedup.relevant_memory_bytes(), 0);
    assert!(!dedup.is_loaded("/a.rs"));
}
