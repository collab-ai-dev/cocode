use super::KairosRolloverWatcher;
use chrono::Datelike;
use chrono::Local;
use chrono::NaiveDate;
use chrono::TimeZone;
use pretty_assertions::assert_eq;

fn millis_for(date: NaiveDate, hour: u32) -> i64 {
    let dt = Local
        .with_ymd_and_hms(date.year(), date.month(), date.day(), hour, 0, 0)
        .single()
        .expect("hour valid");
    dt.timestamp_millis()
}

#[test]
fn first_tick_seeds_and_returns_none() {
    let w = KairosRolloverWatcher::new();
    let today = NaiveDate::from_ymd_opt(2026, 5, 19).unwrap();
    let out = w.tick(millis_for(today, 10));
    assert_eq!(out, None);
}

#[test]
fn same_day_tick_returns_none() {
    let w = KairosRolloverWatcher::new();
    let today = NaiveDate::from_ymd_opt(2026, 5, 19).unwrap();
    w.seed(today);
    let later_same_day = millis_for(today, 23);
    assert_eq!(w.tick(later_same_day), None);
}

#[test]
fn next_day_tick_returns_yesterday_and_advances_latch() {
    let w = KairosRolloverWatcher::new();
    let day_one = NaiveDate::from_ymd_opt(2026, 5, 19).unwrap();
    let day_two = NaiveDate::from_ymd_opt(2026, 5, 20).unwrap();
    w.seed(day_one);
    let out = w.tick(millis_for(day_two, 1));
    assert_eq!(out, Some(day_one));
    let out2 = w.tick(millis_for(day_two, 8));
    assert_eq!(out2, None);
}

#[test]
fn multi_day_gap_reports_immediately_previous_day_only() {
    let w = KairosRolloverWatcher::new();
    let day_one = NaiveDate::from_ymd_opt(2026, 5, 19).unwrap();
    let day_three = NaiveDate::from_ymd_opt(2026, 5, 21).unwrap();
    w.seed(day_one);
    let out = w.tick(millis_for(day_three, 4));
    assert_eq!(out, Some(day_one));
}
