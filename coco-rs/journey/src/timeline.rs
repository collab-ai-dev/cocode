//! Pure timeline bucketing — Hermes-style adaptive granularity with a
//! recency-driven ink signal. No I/O, no `SystemTime`: the clock (`now_ms`) is
//! injected so the whole module is deterministically testable.
//!
//! Timestamps are bucketed in **UTC** (matching `coco_memory::scan`'s
//! `format_iso_timestamp` precedent) rather than local time — this keeps the
//! module a pure function of its inputs, so labels don't depend on the test
//! machine's timezone.

use chrono::{DateTime, Datelike, NaiveDate, Utc};

use crate::snapshot::{JourneyNode, JourneyNodeBody};

const DAY_MS: i64 = 86_400_000;
/// Below this data span, always keep day resolution (scroll rather than lose
/// it) even if the row count would otherwise force coarser buckets.
const DAY_LOCK_SPAN_MS: i64 = 32 * DAY_MS;
/// Recency floor so the oldest row is dim but still legible ink.
const RECENCY_FLOOR: f32 = 0.06;

/// One timeline row.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineBucket {
    pub start_ms: i64,
    /// `"4 Jul"` / `"Jul 2026"` / `"2026"` (UTC).
    pub label: String,
    pub skills: i32,
    pub memories: i32,
    /// Recency of the newest node in the bucket (0..=1); drives the row's ink.
    pub recency: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Granularity {
    Day,
    Month,
    Year,
}

/// Adaptive-granularity bucketing: try day; if the row count exceeds `max_rows`
/// try month, then year. A data span ≤ 32 days locks to day granularity.
/// `now_ms` is the timeline's right edge ("now"): recency is measured from the
/// oldest node to `now`, so rows near now render bright and old rows dim.
pub fn bucketize(nodes: &[JourneyNode], max_rows: usize, now_ms: i64) -> Vec<TimelineBucket> {
    if nodes.is_empty() {
        return Vec::new();
    }
    let min_ts = nodes.iter().map(|n| n.last_activity_ms).min().unwrap_or(0);
    let max_ts = nodes.iter().map(|n| n.last_activity_ms).max().unwrap_or(0);
    // Right edge = now, or the newest node if the clock is somehow behind it.
    let hi = now_ms.max(max_ts);
    let span = hi.saturating_sub(min_ts);

    let day = build_buckets(nodes, Granularity::Day, min_ts, hi);
    if span <= DAY_LOCK_SPAN_MS || day.len() <= max_rows {
        day
    } else {
        let month = build_buckets(nodes, Granularity::Month, min_ts, hi);
        if month.len() <= max_rows {
            month
        } else {
            build_buckets(nodes, Granularity::Year, min_ts, hi)
        }
    }
}

/// Day-granularity UTC label for a single timestamp (e.g. `"15 Jul"`). Used by
/// the host to stamp each node's display date without pulling `chrono` into the
/// TUI layer.
pub fn day_label(ms: i64) -> String {
    bucket_label(Granularity::Day, bucket_start(Granularity::Day, ms))
}

/// Busiest calendar day (day-granularity label, node count), independent of the
/// display bucketing. `None` for an empty timeline; ties resolve to the
/// earliest day.
pub(crate) fn busiest_day(nodes: &[JourneyNode]) -> Option<(String, i32)> {
    if nodes.is_empty() {
        return None;
    }
    let mut counts: std::collections::BTreeMap<i64, i32> = std::collections::BTreeMap::new();
    for node in nodes {
        *counts
            .entry(bucket_start(Granularity::Day, node.last_activity_ms))
            .or_insert(0) += 1;
    }
    // BTreeMap iterates ascending by start_ms, so `>` keeps the earliest of a
    // tie as the running winner.
    let (start, count) =
        counts.into_iter().fold(
            (0i64, 0i32),
            |acc, (s, c)| {
                if c > acc.1 { (s, c) } else { acc }
            },
        );
    Some((bucket_label(Granularity::Day, start), count))
}

/// Build sorted buckets at one granularity, then assign each bucket's recency.
fn build_buckets(
    nodes: &[JourneyNode],
    gran: Granularity,
    lo: i64,
    hi: i64,
) -> Vec<TimelineBucket> {
    use std::collections::BTreeMap;
    // start_ms -> (skills, memories, newest_ts)
    let mut groups: BTreeMap<i64, (i32, i32, i64)> = BTreeMap::new();
    for node in nodes {
        let start = bucket_start(gran, node.last_activity_ms);
        let entry = groups.entry(start).or_insert((0, 0, i64::MIN));
        match &node.body {
            JourneyNodeBody::AgentSkill { .. } | JourneyNodeBody::UserSkill { .. } => entry.0 += 1,
            JourneyNodeBody::Memory { .. } => entry.1 += 1,
        }
        entry.2 = entry.2.max(node.last_activity_ms);
    }

    let span = hi.saturating_sub(lo);
    let n = groups.len();
    groups
        .into_iter()
        .enumerate()
        .map(|(i, (start, (skills, memories, newest)))| {
            let recency = if span <= 0 {
                // Single-instant span: linear mapping is undefined; fall back to
                // an ordinal ramp so rows still read oldest→newest.
                ordinal_recency(i, n)
            } else {
                let frac = (newest.saturating_sub(lo)) as f32 / span as f32;
                (RECENCY_FLOOR + frac * (1.0 - RECENCY_FLOOR)).clamp(RECENCY_FLOOR, 1.0)
            };
            TimelineBucket {
                start_ms: start,
                label: bucket_label(gran, start),
                skills,
                memories,
                recency,
            }
        })
        .collect()
}

/// Ordinal recency ramp for a single-instant span: `(i+1)/n` over `[.., 1.0]`.
fn ordinal_recency(index: usize, count: usize) -> f32 {
    if count <= 1 {
        return 1.0;
    }
    let frac = (index + 1) as f32 / count as f32;
    (RECENCY_FLOOR + frac * (1.0 - RECENCY_FLOOR)).clamp(RECENCY_FLOOR, 1.0)
}

/// UTC bucket start for a timestamp at the given granularity.
fn bucket_start(gran: Granularity, ms: i64) -> i64 {
    let Some(dt) = DateTime::<Utc>::from_timestamp_millis(ms) else {
        return ms;
    };
    let date = dt.date_naive();
    let start_date = match gran {
        Granularity::Day => date,
        Granularity::Month => NaiveDate::from_ymd_opt(date.year(), date.month(), 1).unwrap_or(date),
        Granularity::Year => NaiveDate::from_ymd_opt(date.year(), 1, 1).unwrap_or(date),
    };
    start_date
        .and_hms_opt(0, 0, 0)
        .map(|ndt| ndt.and_utc().timestamp_millis())
        .unwrap_or(ms)
}

/// Human label for a bucket start (UTC), built from components (not strftime)
/// so it is locale- and platform-independent.
fn bucket_label(gran: Granularity, start_ms: i64) -> String {
    let Some(dt) = DateTime::<Utc>::from_timestamp_millis(start_ms) else {
        return String::new();
    };
    let date = dt.date_naive();
    match gran {
        Granularity::Day => format!("{} {}", date.day(), month_abbr(date.month())),
        Granularity::Month => format!("{} {}", month_abbr(date.month()), date.year()),
        Granularity::Year => date.year().to_string(),
    }
}

fn month_abbr(month: u32) -> &'static str {
    match month {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => "???",
    }
}

#[cfg(test)]
#[path = "timeline.test.rs"]
mod tests;
