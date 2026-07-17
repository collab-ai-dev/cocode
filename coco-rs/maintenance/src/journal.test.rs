use super::*;
use pretty_assertions::assert_eq;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Rec {
    n: i64,
    s: String,
}

#[test]
fn test_append_read_roundtrip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("j.jsonl");
    for n in 0..3 {
        append_jsonl(
            &path,
            &Rec {
                n,
                s: format!("v{n}"),
            },
        );
    }
    let read: Vec<Rec> = read_jsonl(&path);
    assert_eq!(read.len(), 3);
    assert_eq!(read[0].n, 0);
    assert_eq!(read[2].s, "v2");
}

#[test]
fn test_read_missing_file_is_empty() {
    let dir = tempfile::tempdir().expect("tempdir");
    let read: Vec<Rec> = read_jsonl(&dir.path().join("nope.jsonl"));
    assert!(read.is_empty());
}

#[test]
fn test_corrupt_line_skipped() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("j.jsonl");
    append_jsonl(
        &path,
        &Rec {
            n: 1,
            s: "ok".into(),
        },
    );
    // Inject a torn / non-JSON line in the middle.
    std::fs::write(
        &path,
        "{\"n\":1,\"s\":\"ok\"}\n{ this is not json\n{\"n\":2,\"s\":\"also-ok\"}\n",
    )
    .expect("write");
    let read: Vec<Rec> = read_jsonl(&path);
    assert_eq!(read.len(), 2);
    assert_eq!(read[0].n, 1);
    assert_eq!(read[1].n, 2);
}

#[test]
fn test_concurrent_append_all_lines_intact() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = Arc::new(dir.path().join("j.jsonl"));
    let mut handles = Vec::new();
    for t in 0..4 {
        let p = path.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..25 {
                append_jsonl(
                    &p,
                    &Rec {
                        n: t * 100 + i,
                        s: "x".into(),
                    },
                );
            }
        }));
    }
    for h in handles {
        h.join().expect("join");
    }
    let read: Vec<Rec> = read_jsonl(&path);
    // Every line parses (no torn interleave on a local fs).
    assert_eq!(read.len(), 100);
}

#[test]
fn test_rotate_when_over_renames_and_truncates() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("j.jsonl");
    for n in 0..50 {
        append_jsonl(
            &path,
            &Rec {
                n,
                s: "padding-content".into(),
            },
        );
    }
    let before = std::fs::metadata(&path).expect("meta").len();
    assert!(before > 100);
    rotate_if_over(&path, 100);
    // Original path is gone (renamed); `.1` holds the old content.
    assert!(!path.exists());
    let mut rotated = path.as_os_str().to_owned();
    rotated.push(".1");
    assert!(std::path::Path::new(&rotated).exists());
    // A subsequent append recreates the primary file fresh.
    append_jsonl(
        &path,
        &Rec {
            n: 999,
            s: "fresh".into(),
        },
    );
    let read: Vec<Rec> = read_jsonl(&path);
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].n, 999);
}

#[test]
fn test_rotate_no_op_under_threshold() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("j.jsonl");
    append_jsonl(
        &path,
        &Rec {
            n: 1,
            s: "s".into(),
        },
    );
    rotate_if_over(&path, 1_000_000);
    assert!(path.exists());
    let read: Vec<Rec> = read_jsonl(&path);
    assert_eq!(read.len(), 1);
}

#[test]
fn append_rotating_bounds_growth_at_the_ceiling() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("j.jsonl");
    // One record just over the ceiling, so the NEXT append must rotate it away.
    let huge = "x".repeat(DEFAULT_MAX_BYTES as usize + 1);
    append_rotating(&path, &Rec { n: 1, s: huge });
    assert!(
        std::fs::metadata(&path).expect("meta").len() > DEFAULT_MAX_BYTES,
        "precondition: file is over the ceiling"
    );

    append_rotating(
        &path,
        &Rec {
            n: 2,
            s: "after".into(),
        },
    );

    // Rotation is bundled into the append, so a caller cannot grow the journal
    // without bound by forgetting a separate rotate call.
    let read: Vec<Rec> = read_jsonl(&path);
    assert_eq!(read.len(), 1, "oversized generation was rotated away");
    assert_eq!(read[0].n, 2);
    let mut rotated = path.as_os_str().to_owned();
    rotated.push(".1");
    assert!(
        std::path::Path::new(&rotated).exists(),
        "previous generation is retained as .1"
    );
}

#[test]
fn append_rotating_keeps_history_under_the_ceiling() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("j.jsonl");
    for n in 0..5 {
        append_rotating(
            &path,
            &Rec {
                n,
                s: "small".into(),
            },
        );
    }
    let read: Vec<Rec> = read_jsonl(&path);
    assert_eq!(read.len(), 5, "no rotation while comfortably under the cap");
}
