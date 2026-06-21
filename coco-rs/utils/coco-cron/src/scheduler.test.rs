use super::*;
use chrono::Local;
use chrono::TimeZone;

/// Epoch ms for a local wall-clock time (tests are tz-independent: anchors and
/// `now` are built the same way, so relative cron logic holds anywhere).
fn ms(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
    Local
        .with_ymd_and_hms(y, mo, d, h, mi, 0)
        .single()
        .expect("valid local time")
        .timestamp_millis()
}

fn timing<'a>(
    id: &'a str,
    cron: &'a str,
    created: i64,
    last_fired: Option<i64>,
    recurring: bool,
) -> CronTiming<'a> {
    CronTiming {
        id,
        cron,
        created_at_ms: created,
        last_fired_at_ms: last_fired,
        recurring,
        permanent: false,
    }
}

#[test]
fn tick_fires_recurring_once_when_due_then_reschedules() {
    let created = ms(2025, 1, 1, 8, 0);
    let tasks = vec![timing("a", "0 9 * * *", created, None, true)];
    let mut st = CronTickState::new();

    // First sight at 08:30: anchored from created (08:00) -> next 09:00, not yet due.
    assert!(
        st.tick(&tasks, ms(2025, 1, 1, 8, 30), RECURRING_MAX_AGE_MS)
            .is_empty()
    );
    // At 09:00 it fires exactly once.
    let fires = st.tick(&tasks, ms(2025, 1, 1, 9, 0), RECURRING_MAX_AGE_MS);
    assert_eq!(fires.len(), 1);
    assert_eq!(fires[0].id, "a");
    assert!(fires[0].recurring && !fires[0].aged);
    // A minute later it does NOT re-fire (rescheduled to tomorrow).
    assert!(
        st.tick(&tasks, ms(2025, 1, 1, 9, 1), RECURRING_MAX_AGE_MS)
            .is_empty()
    );
    assert_eq!(st.next_fire_time(), Some(ms(2025, 1, 2, 9, 0)));
}

#[test]
fn tick_fires_recurring_missed_on_first_sight() {
    // Created 3h before "now" with an hourly cron, never fired -> first tick
    // fires once (TS anchors recurring first-sight from created_at).
    let created = ms(2025, 1, 1, 6, 0);
    let now = ms(2025, 1, 1, 9, 30);
    let tasks = vec![timing("a", "0 * * * *", created, None, true)];
    let mut st = CronTickState::new();
    let fires = st.tick(&tasks, now, RECURRING_MAX_AGE_MS);
    assert_eq!(
        fires.len(),
        1,
        "recurring task due-while-down fires once on first tick"
    );
    // Rescheduled from `now` (10:00), not catching up 7:00/8:00/9:00.
    assert_eq!(st.next_fire_time(), Some(ms(2025, 1, 1, 10, 0)));
}

#[test]
fn tick_one_shot_fires_once_and_is_dropped() {
    let created = ms(2025, 1, 1, 8, 0);
    let tasks = vec![timing("a", "0 9 * * *", created, None, false)];
    let mut st = CronTickState::new();
    let fires = st.tick(&tasks, ms(2025, 1, 1, 9, 0), RECURRING_MAX_AGE_MS);
    assert_eq!(fires.len(), 1);
    assert!(!fires[0].recurring);
    // The caller removes the task; the schedule entry was dropped either way.
    assert_eq!(st.next_fire_time(), None);
}

#[test]
fn tick_recurring_ages_out() {
    // Created 8 days ago (> 7d max age) -> fires once with aged=true.
    let created = ms(2025, 1, 1, 9, 0);
    let now = ms(2025, 1, 9, 9, 0); // 8 days later, on a fire boundary
    let tasks = vec![timing(
        "a",
        "0 9 * * *",
        created,
        Some(ms(2025, 1, 8, 9, 0)),
        true,
    )];
    let mut st = CronTickState::new();
    let fires = st.tick(&tasks, now, RECURRING_MAX_AGE_MS);
    assert_eq!(fires.len(), 1);
    assert!(fires[0].aged, "recurring task past max age fires aged");
    assert_eq!(st.next_fire_time(), None, "aged task dropped from schedule");
}

#[test]
fn tick_zero_max_age_never_ages() {
    let created = ms(2025, 1, 1, 9, 0);
    let now = ms(2025, 2, 1, 9, 0);
    let tasks = vec![timing(
        "a",
        "0 9 * * *",
        created,
        Some(ms(2025, 1, 31, 9, 0)),
        true,
    )];
    let mut st = CronTickState::new();
    let fires = st.tick(&tasks, now, /*max_age*/ 0);
    assert_eq!(fires.len(), 1);
    assert!(!fires[0].aged);
}

#[test]
fn tick_evicts_vanished_tasks() {
    let created = ms(2025, 1, 1, 8, 0);
    let mut st = CronTickState::new();
    let tasks = vec![timing("a", "0 9 * * *", created, None, true)];
    st.tick(&tasks, ms(2025, 1, 1, 8, 30), RECURRING_MAX_AGE_MS);
    assert!(st.next_fire_time().is_some());
    // Task removed from the set -> its schedule entry is evicted.
    st.tick(&[], ms(2025, 1, 1, 8, 31), RECURRING_MAX_AGE_MS);
    assert_eq!(st.next_fire_time(), None);
}

#[test]
fn find_missed_returns_overdue_one_shots_only() {
    let now = ms(2025, 1, 2, 12, 0);
    let tasks = vec![
        // one-shot whose 09:00 fire passed yesterday, never fired -> missed
        timing("missed", "0 9 1 1 *", ms(2025, 1, 1, 0, 0), None, false),
        // recurring -> excluded (tick handles it)
        timing("recur", "0 9 * * *", ms(2025, 1, 1, 0, 0), None, true),
        // one-shot already fired -> excluded
        timing(
            "done",
            "0 9 1 1 *",
            ms(2025, 1, 1, 0, 0),
            Some(ms(2025, 1, 1, 9, 0)),
            false,
        ),
        // future one-shot -> not missed
        timing("future", "0 9 1 6 *", now, None, false),
    ];
    assert_eq!(find_missed(&tasks, now), vec!["missed".to_string()]);
}
