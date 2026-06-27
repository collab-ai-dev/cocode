use super::*;
use chrono::TimeZone;

// ── parse_cron_expression ────────────────────────────────────────────

#[test]
fn parse_wildcards_and_steps() {
    let f = parse_cron_expression("*/5 * * * *").unwrap();
    assert_eq!(f.minute, vec![0, 5, 10, 15, 20, 25, 30, 35, 40, 45, 50, 55]);
    assert_eq!(f.hour.len(), 24);
    assert_eq!(f.day_of_month, (1..=31).collect::<Vec<_>>());
    assert_eq!(f.month, (1..=12).collect::<Vec<_>>());
    assert_eq!(f.day_of_week, (0..=6).collect::<Vec<_>>());
}

#[test]
fn parse_range_list_single() {
    let f = parse_cron_expression("0,30 9-17 * * 1-5").unwrap();
    assert_eq!(f.minute, vec![0, 30]);
    assert_eq!(f.hour, vec![9, 10, 11, 12, 13, 14, 15, 16, 17]);
    assert_eq!(f.day_of_week, vec![1, 2, 3, 4, 5]);
}

#[test]
fn parse_seven_is_sunday_alias() {
    assert_eq!(
        parse_cron_expression("* * * * 7").unwrap().day_of_week,
        vec![0]
    );
    // 5-7 = Fri,Sat,Sun -> [0,5,6] (sorted, 7 folded to 0)
    assert_eq!(
        parse_cron_expression("0 0 * * 5-7").unwrap().day_of_week,
        vec![0, 5, 6]
    );
}

#[test]
fn parse_range_step() {
    // 0-30/10 minute -> 0,10,20,30
    assert_eq!(
        parse_cron_expression("0-30/10 * * * *").unwrap().minute,
        vec![0, 10, 20, 30]
    );
}

#[test]
fn parse_rejects_invalid() {
    assert!(parse_cron_expression("* * * *").is_none()); // 4 fields
    assert!(parse_cron_expression("* * * * * *").is_none()); // 6 fields
    assert!(parse_cron_expression("60 * * * *").is_none()); // minute out of range
    assert!(parse_cron_expression("* 24 * * *").is_none()); // hour out of range
    assert!(parse_cron_expression("* * 0 * *").is_none()); // dom min is 1
    assert!(parse_cron_expression("* * * 13 *").is_none()); // month out of range
    assert!(parse_cron_expression("*/0 * * * *").is_none()); // zero step
    assert!(parse_cron_expression("5-1 * * * *").is_none()); // reversed range
    assert!(parse_cron_expression("abc * * * *").is_none()); // non-numeric
    assert!(parse_cron_expression("").is_none());
}

// ── compute_next_cron_run (local time) ───────────────────────────────

fn local(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
    Local
        .with_ymd_and_hms(y, mo, d, h, mi, 0)
        .single()
        .expect("valid local time")
}

#[test]
fn next_daily_same_day_and_rollover() {
    let f = parse_cron_expression("0 9 * * *").unwrap();
    // Before 9am -> 9am same day.
    assert_eq!(
        compute_next_cron_run(&f, local(2025, 1, 1, 8, 0)),
        Some(local(2025, 1, 1, 9, 0))
    );
    // After 9am -> 9am next day.
    assert_eq!(
        compute_next_cron_run(&f, local(2025, 1, 1, 10, 0)),
        Some(local(2025, 1, 2, 9, 0))
    );
    // Exactly at 9:00 -> strictly after -> next day.
    assert_eq!(
        compute_next_cron_run(&f, local(2025, 1, 1, 9, 0)),
        Some(local(2025, 1, 2, 9, 0))
    );
}

#[test]
fn next_hourly() {
    let f = parse_cron_expression("0 * * * *").unwrap();
    assert_eq!(
        compute_next_cron_run(&f, local(2025, 3, 4, 8, 30)),
        Some(local(2025, 3, 4, 9, 0))
    );
}

#[test]
fn next_day_of_week() {
    // Sundays at 00:00. 2025-01-01 is a Wednesday; next Sunday is 2025-01-05.
    let f = parse_cron_expression("0 0 * * 0").unwrap();
    assert_eq!(
        compute_next_cron_run(&f, local(2025, 1, 1, 12, 0)),
        Some(local(2025, 1, 5, 0, 0))
    );
}

#[test]
fn next_dom_or_dow_semantics() {
    // Both constrained -> OR: dom=15 OR Monday. 2025-01-01 is Wed; first
    // match after is Mon 2025-01-06 (a Monday before the 15th).
    let f = parse_cron_expression("0 0 15 * 1").unwrap();
    assert_eq!(
        compute_next_cron_run(&f, local(2025, 1, 1, 0, 0)),
        Some(local(2025, 1, 6, 0, 0))
    );
}

#[test]
fn unreachable_returns_none() {
    // Feb 30 never exists, day-of-week wild -> no match in 366 days.
    let f = parse_cron_expression("0 0 30 2 *").unwrap();
    assert_eq!(compute_next_cron_run(&f, local(2025, 1, 1, 0, 0)), None);
}

#[test]
fn next_cron_run_ms_roundtrip() {
    let from = local(2025, 1, 1, 8, 0).timestamp_millis();
    let next = next_cron_run_ms("0 9 * * *", from).unwrap();
    assert_eq!(next, local(2025, 1, 1, 9, 0).timestamp_millis());
    assert!(next_cron_run_ms("0 0 30 2 *", from).is_none());
    assert!(next_cron_run_ms("not a cron", from).is_none());
}

// ── cron_to_human ────────────────────────────────────────────────────

#[test]
fn human_readable_patterns() {
    assert_eq!(cron_to_human("*/5 * * * *"), "Every 5 minutes");
    assert_eq!(cron_to_human("*/1 * * * *"), "Every minute");
    assert_eq!(cron_to_human("0 * * * *"), "Every hour");
    assert_eq!(cron_to_human("15 * * * *"), "Every hour at :15");
    assert_eq!(cron_to_human("0 */2 * * *"), "Every 2 hours");
    assert_eq!(cron_to_human("0 9 * * *"), "Every day at 9:00 AM");
    assert_eq!(cron_to_human("30 14 * * *"), "Every day at 2:30 PM");
    assert_eq!(cron_to_human("0 0 * * *"), "Every day at 12:00 AM");
    assert_eq!(cron_to_human("0 9 * * 1"), "Every Monday at 9:00 AM");
    assert_eq!(cron_to_human("0 9 * * 7"), "Every Sunday at 9:00 AM");
    assert_eq!(cron_to_human("30 8 * * 1-5"), "Weekdays at 8:30 AM");
    // Falls through to raw for unsupported shapes.
    assert_eq!(cron_to_human("0 0 15 * *"), "0 0 15 * *");
    assert_eq!(cron_to_human("bad"), "bad");
}
