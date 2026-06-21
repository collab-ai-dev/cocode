use super::*;
use crate::generator::GeneratorContext;
use crate::types::ReminderOutput;
use coco_config::SystemReminderConfig;
use pretty_assertions::assert_eq;

fn cfg() -> SystemReminderConfig {
    SystemReminderConfig::default()
}

// ── NestedMemoryGenerator ──

#[tokio::test]
async fn nested_memory_skips_when_empty() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .nested_memories(vec![])
        .build();
    assert!(
        NestedMemoryGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn nested_memory_skips_when_all_entries_empty_content() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .nested_memories(vec![NestedMemoryInfo {
            path: "/tmp/CLAUDE.md".into(),
            content: String::new(),
        }])
        .build();
    assert!(
        NestedMemoryGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn nested_memory_emits_ts_template_for_single_entry() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .nested_memories(vec![NestedMemoryInfo {
            path: "/repo/CLAUDE.md".into(),
            content: "coding rules here".into(),
        }])
        .build();
    let r = NestedMemoryGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .expect("emits");
    let text = r.content().unwrap();
    // Expected format: "Contents of {path}:\n\n{content}"
    assert_eq!(text, "Contents of /repo/CLAUDE.md:\n\ncoding rules here");
}

#[tokio::test]
async fn nested_memory_joins_multiple_entries_with_blank_line() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .nested_memories(vec![
            NestedMemoryInfo {
                path: "/repo/CLAUDE.md".into(),
                content: "root rules".into(),
            },
            NestedMemoryInfo {
                path: "/repo/src/CLAUDE.md".into(),
                content: "src rules".into(),
            },
        ])
        .build();
    let text = NestedMemoryGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    assert!(text.contains("Contents of /repo/CLAUDE.md:\n\nroot rules"));
    assert!(text.contains("Contents of /repo/src/CLAUDE.md:\n\nsrc rules"));
    assert!(text.contains("root rules\n\nContents of /repo/src/CLAUDE.md"));
}

#[tokio::test]
async fn nested_memory_respects_config_flag() {
    let mut c = cfg();
    c.attachments.nested_memory = false;
    assert!(!NestedMemoryGenerator.is_enabled(&c));
}

// ── RelevantMemoriesGenerator ──

#[tokio::test]
async fn relevant_memories_skips_when_empty() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .relevant_memories(vec![])
        .build();
    assert!(
        RelevantMemoriesGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn relevant_memories_emits_one_message_per_entry() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .relevant_memories(vec![
            RelevantMemoryInfo {
                path: "/m/a.md".into(),
                content: "a content".into(),
                mtime_ms: 1,
                header: Some("Memory: a.md (1 hour ago)".into()),
            },
            RelevantMemoryInfo {
                path: "/m/b.md".into(),
                content: "b content".into(),
                mtime_ms: 2,
                header: Some("Memory: b.md (2 hours ago)".into()),
            },
        ])
        .build();
    let r = RelevantMemoriesGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .expect("emits");
    match r.output {
        ReminderOutput::Messages(msgs) => {
            assert_eq!(msgs.len(), 2);
            let m0 = match &msgs[0].blocks[0] {
                crate::types::ContentBlock::Text { text } => text,
                _ => panic!("expected text block"),
            };
            assert!(m0.starts_with("Memory: a.md (1 hour ago)"));
            assert!(m0.contains("a content"));
        }
        other => panic!("expected Messages output, got {other:?}"),
    }
}

#[tokio::test]
async fn relevant_memories_falls_back_to_path_header_when_none() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .relevant_memories(vec![RelevantMemoryInfo {
            path: "/m/x.md".into(),
            content: "content x".into(),
            mtime_ms: 0,
            header: None,
        }])
        .build();
    let r = RelevantMemoriesGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap();
    let ReminderOutput::Messages(msgs) = &r.output else {
        panic!("expected Messages");
    };
    let crate::types::ContentBlock::Text { text } = &msgs[0].blocks[0] else {
        panic!("expected text");
    };
    assert!(text.starts_with("Memory: /m/x.md"));
    assert!(text.contains("content x"));
}

#[tokio::test]
async fn relevant_memories_skips_empty_content_entries() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .relevant_memories(vec![RelevantMemoryInfo {
            path: "/m/empty.md".into(),
            content: String::new(),
            mtime_ms: 0,
            header: Some("Memory: empty.md".into()),
        }])
        .build();
    assert!(
        RelevantMemoriesGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn relevant_memories_respects_config_flag() {
    let mut c = cfg();
    c.attachments.relevant_memories = false;
    assert!(!RelevantMemoriesGenerator.is_enabled(&c));
}
