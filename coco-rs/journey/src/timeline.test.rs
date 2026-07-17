use super::*;
use crate::snapshot::{JourneyNode, JourneyNodeBody};
use chrono::{NaiveDate, Utc};
use coco_skills::telemetry::SkillTelemetryStats;
use pretty_assertions::assert_eq;

fn ms(y: i32, m: u32, d: u32) -> i64 {
    NaiveDate::from_ymd_opt(y, m, d)
        .unwrap()
        .and_hms_opt(12, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis()
}

fn skill_node(last: i64) -> JourneyNode {
    JourneyNode {
        title: "s".into(),
        description: String::new(),
        first_seen_ms: last,
        last_activity_ms: last,
        body: JourneyNodeBody::UserSkill {
            path: "s".into(),
            telemetry: SkillTelemetryStats::default(),
        },
        history: Vec::new(),
    }
}

fn mem_node(last: i64) -> JourneyNode {
    JourneyNode {
        title: "m".into(),
        description: String::new(),
        first_seen_ms: last,
        last_activity_ms: last,
        body: JourneyNodeBody::Memory {
            filename: "m.md".into(),
        },
        history: Vec::new(),
    }
}

#[test]
fn test_empty_yields_no_buckets() {
    assert!(bucketize(&[], 10, ms(2026, 7, 15)).is_empty());
}

#[test]
fn test_single_node_day_bucket() {
    let nodes = [skill_node(ms(2026, 7, 4))];
    let buckets = bucketize(&nodes, 10, ms(2026, 7, 4));
    assert_eq!(buckets.len(), 1);
    assert_eq!(buckets[0].label, "4 Jul");
    assert_eq!(buckets[0].skills, 1);
    assert_eq!(buckets[0].memories, 0);
}

#[test]
fn test_day_granularity_locks_under_32_days() {
    // 5 distinct days spanning < 32 days; even a small max_rows keeps day.
    let nodes: Vec<JourneyNode> = (4..=8).map(|d| skill_node(ms(2026, 7, d))).collect();
    let buckets = bucketize(&nodes, 2, ms(2026, 7, 8));
    assert_eq!(buckets.len(), 5);
    assert!(buckets.iter().all(|b| b.label.contains("Jul")));
}

#[test]
fn test_span_boundary_32_vs_33_days() {
    // Exactly 32-day span → still day-locked regardless of row count.
    let lo = ms(2026, 6, 1);
    let nodes = [skill_node(lo), skill_node(ms(2026, 7, 3))]; // Jun 1 → Jul 3 = 32 days
    let day = bucketize(&nodes, 1, ms(2026, 7, 3));
    assert_eq!(day.len(), 2, "32-day span locks to day");

    // 40-day span with more day-buckets than max_rows → coarsen to month.
    let nodes: Vec<JourneyNode> = (0..6)
        .map(|k| skill_node(ms(2026, 5, 1) + k * 8 * 86_400_000))
        .collect();
    let coarse = bucketize(&nodes, 3, ms(2026, 7, 1));
    assert!(coarse.len() <= 3, "coarsened to fit max_rows");
    assert!(coarse.iter().all(|b| b.label.contains("2026")));
}

#[test]
fn test_month_overflow_to_year() {
    // 15 distinct months, max_rows 3 → months (15) > 3 → fall to year.
    let mut nodes = Vec::new();
    for k in 0..15i32 {
        let y = 2025 + k / 12;
        let m = 1 + (k % 12) as u32;
        nodes.push(skill_node(ms(y, m, 15)));
    }
    let buckets = bucketize(&nodes, 3, ms(2026, 6, 1));
    // Two years present → year granularity.
    assert!(buckets.iter().all(|b| b.label.len() == 4), "year labels");
    assert!(buckets.len() <= 3);
}

#[test]
fn test_recency_older_is_dimmer() {
    let nodes = [skill_node(ms(2026, 1, 1)), skill_node(ms(2026, 7, 1))];
    let buckets = bucketize(&nodes, 50, ms(2026, 7, 1));
    assert_eq!(buckets.len(), 2);
    assert!(
        buckets[0].recency < buckets[1].recency,
        "older bucket dimmer: {buckets:?}"
    );
    // Newest bucket at `now` → maximal ink.
    assert!((buckets[1].recency - 1.0).abs() < 1e-6);
    // Oldest bucket floored, never below the floor.
    assert!(buckets[0].recency >= 0.06);
}

#[test]
fn test_single_instant_span_ordinal_fallback() {
    let t = ms(2026, 7, 4);
    let nodes = [skill_node(t)];
    // now == the only instant → span 0 → ordinal fallback → single row = 1.0.
    let buckets = bucketize(&nodes, 10, t);
    assert_eq!(buckets.len(), 1);
    assert!((buckets[0].recency - 1.0).abs() < 1e-6);
}

#[test]
fn test_skill_and_memory_counts_per_bucket() {
    let day = ms(2026, 7, 4);
    let nodes = [skill_node(day), mem_node(day), mem_node(day)];
    let buckets = bucketize(&nodes, 10, day);
    assert_eq!(buckets.len(), 1);
    assert_eq!(buckets[0].skills, 1);
    assert_eq!(buckets[0].memories, 2);
}

#[test]
fn test_busiest_day() {
    let nodes = [
        skill_node(ms(2026, 7, 4)),
        skill_node(ms(2026, 7, 15)),
        mem_node(ms(2026, 7, 15)),
        mem_node(ms(2026, 7, 15)),
    ];
    let (label, count) = busiest_day(&nodes).unwrap();
    assert_eq!(label, "15 Jul");
    assert_eq!(count, 3);
}

#[test]
fn test_busiest_day_empty_is_none() {
    assert_eq!(busiest_day(&[]), None);
}

#[test]
fn test_labels_have_no_control_chars() {
    let nodes = [skill_node(ms(2026, 12, 25)), skill_node(ms(2026, 1, 1))];
    for b in bucketize(&nodes, 50, Utc::now().timestamp_millis()) {
        assert!(b.label.chars().all(|c| !c.is_control()));
    }
}
