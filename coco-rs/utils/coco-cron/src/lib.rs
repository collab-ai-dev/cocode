//! Minimal cron expression parsing and next-run calculation.
//!
//! Faithful port of the TS `utils/cron.ts`. Supports the standard 5-field
//! cron subset:
//!
//! ```text
//! minute hour day-of-month month day-of-week
//! ```
//!
//! Field syntax: wildcard, `N`, step (`*/N`), range (`N-M`, `N-M/S`), list
//! (`N,M,...`). No `L`, `W`, `?`, or name aliases. All times are interpreted
//! in the process's **local** timezone — `"0 9 * * *"` means 9am wherever the
//! CLI is running.

use chrono::DateTime;
use chrono::Datelike;
use chrono::Duration;
use chrono::Local;
use chrono::TimeZone;
use chrono::Timelike;
use std::collections::BTreeSet;

pub mod scheduler;
pub use scheduler::{
    CronTickState, CronTiming, DueFire, RECURRING_MAX_AGE_MS, find_missed, is_recurring_task_aged,
};

/// A parsed 5-field cron expression, each field expanded into the sorted set of
/// matching values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronFields {
    pub minute: Vec<u32>,
    pub hour: Vec<u32>,
    pub day_of_month: Vec<u32>,
    pub month: Vec<u32>,
    /// 0 = Sunday .. 6 = Saturday (7 is accepted on input as a Sunday alias).
    pub day_of_week: Vec<u32>,
}

/// `(min, max)` inclusive range per field, in cron field order.
const FIELD_RANGES: [(u32, u32); 5] = [
    (0, 59), // minute
    (0, 23), // hour
    (1, 31), // dayOfMonth
    (1, 12), // month
    (0, 6),  // dayOfWeek (0=Sunday; 7 accepted as Sunday alias)
];

/// Expand a single cron field into a sorted vec of matching values, or `None`
/// if invalid. Supports wildcard, `*/N` step, `N-M[/S]` range, and `,` lists.
fn expand_field(field: &str, min: u32, max: u32) -> Option<Vec<u32>> {
    // dayOfWeek accepts 7 as a Sunday alias (mapped to 0), both as a single
    // value and in ranges (e.g. `5-7` = Fri,Sat,Sun -> [5,6,0]).
    let is_dow = min == 0 && max == 6;
    let mut out: BTreeSet<u32> = BTreeSet::new();

    for part in field.split(',') {
        // wildcard or step: `*` / `*/N`
        if part == "*" {
            for i in min..=max {
                out.insert(i);
            }
            continue;
        }
        if let Some(step_str) = part.strip_prefix("*/") {
            let step: u32 = step_str.parse().ok()?;
            if step < 1 {
                return None;
            }
            let mut i = min;
            while i <= max {
                out.insert(i);
                i += step;
            }
            continue;
        }

        // range: `N-M` or `N-M/S`. Require a digit before '-' (a leading '-'
        // means a malformed/negative value, which TS's regex also rejects).
        if let Some(dash) = part.find('-')
            && dash > 0
        {
            let lo_str = &part[..dash];
            let (hi_str, step) = match part[dash + 1..].split_once('/') {
                Some((hi, step_str)) => {
                    let step: u32 = step_str.parse().ok()?;
                    if step < 1 {
                        return None;
                    }
                    (hi, step)
                }
                None => (&part[dash + 1..], 1),
            };
            let lo: u32 = lo_str.parse().ok()?;
            let hi: u32 = hi_str.parse().ok()?;
            let eff_max = if is_dow { 7 } else { max };
            if lo > hi || lo < min || hi > eff_max {
                return None;
            }
            let mut i = lo;
            while i <= hi {
                out.insert(if is_dow && i == 7 { 0 } else { i });
                i += step;
            }
            continue;
        }

        // plain N
        match part.parse::<u32>() {
            Ok(mut n) => {
                if is_dow && n == 7 {
                    n = 0;
                }
                if n < min || n > max {
                    return None;
                }
                out.insert(n);
            }
            Err(_) => return None,
        }
    }

    if out.is_empty() {
        return None;
    }
    Some(out.into_iter().collect())
}

/// Parse a 5-field cron expression into expanded number sets. Returns `None`
/// for invalid or unsupported syntax.
pub fn parse_cron_expression(expr: &str) -> Option<CronFields> {
    let parts: Vec<&str> = expr.split_whitespace().collect();
    if parts.len() != 5 {
        return None;
    }
    Some(CronFields {
        minute: expand_field(parts[0], FIELD_RANGES[0].0, FIELD_RANGES[0].1)?,
        hour: expand_field(parts[1], FIELD_RANGES[1].0, FIELD_RANGES[1].1)?,
        day_of_month: expand_field(parts[2], FIELD_RANGES[2].0, FIELD_RANGES[2].1)?,
        month: expand_field(parts[3], FIELD_RANGES[3].0, FIELD_RANGES[3].1)?,
        day_of_week: expand_field(parts[4], FIELD_RANGES[4].0, FIELD_RANGES[4].1)?,
    })
}

/// `true` if `expr` is a valid 5-field cron expression.
pub fn is_valid_cron_expression(expr: &str) -> bool {
    parse_cron_expression(expr).is_some()
}

/// Compute the next local instant strictly after `from` that matches `fields`,
/// or `None` if there is no match in the next 366 days.
///
/// Standard cron semantics: when both dayOfMonth and dayOfWeek are constrained
/// (neither is its full range), a date matches if EITHER matches.
///
/// The walk steps one real minute at a time on the underlying instant, so DST
/// is handled naturally: a fixed-hour cron whose target lands in a
/// spring-forward gap (e.g. `30 2 * * *`) is skipped that day (the local hour
/// never appears); wildcard-hour crons fire at the first valid minute after the
/// gap. This matches vixie-cron behavior.
pub fn compute_next_cron_run(
    fields: &CronFields,
    from: DateTime<Local>,
) -> Option<DateTime<Local>> {
    let dom_wild = fields.day_of_month.len() == 31;
    let dow_wild = fields.day_of_week.len() == 7;

    // Round up to the next whole minute, strictly after `from`.
    let mut t = from
        .with_second(0)?
        .with_nanosecond(0)?
        .checked_add_signed(Duration::minutes(1))?;

    let max_iter = 366 * 24 * 60;
    for _ in 0..max_iter {
        let month = t.month(); // 1-12
        let dom = t.day(); // 1-31
        let dow = t.weekday().num_days_from_sunday(); // 0=Sun..6=Sat
        let day_matches = if dom_wild && dow_wild {
            true
        } else if dom_wild {
            fields.day_of_week.contains(&dow)
        } else if dow_wild {
            fields.day_of_month.contains(&dom)
        } else {
            fields.day_of_month.contains(&dom) || fields.day_of_week.contains(&dow)
        };

        if fields.month.contains(&month)
            && day_matches
            && fields.hour.contains(&t.hour())
            && fields.minute.contains(&t.minute())
        {
            return Some(t);
        }
        t = t.checked_add_signed(Duration::minutes(1))?;
    }
    None
}

/// Next fire time in epoch ms for a cron string, strictly after `from_ms`.
/// Returns `None` if invalid or no match in the next 366 days.
pub fn next_cron_run_ms(cron: &str, from_ms: i64) -> Option<i64> {
    let fields = parse_cron_expression(cron)?;
    let from = Local.timestamp_millis_opt(from_ms).single()?;
    compute_next_cron_run(&fields, from).map(|d| d.timestamp_millis())
}

const DAY_NAMES: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// Format `H:MM AM/PM` (en-US, `hour:'numeric', minute:'2-digit'`).
fn format_local_time(minute: u32, hour: u32) -> String {
    let (h12, ampm) = match hour {
        0 => (12, "AM"),
        1..=11 => (hour, "AM"),
        12 => (12, "PM"),
        _ => (hour - 12, "PM"),
    };
    format!("{h12}:{minute:02} {ampm}")
}

/// Human-readable rendering of a cron string. Intentionally narrow: covers
/// common patterns, falling through to the raw cron string for anything else.
///
/// NOTE: the TS `utc` option (for CCR remote triggers, which run on servers in
/// UTC) is intentionally omitted — that path belongs to the deferred
/// `RemoteTrigger` tool. Local scheduled tasks need only local rendering.
pub fn cron_to_human(cron: &str) -> String {
    let parts: Vec<&str> = cron.split_whitespace().collect();
    if parts.len() != 5 {
        return cron.to_string();
    }
    let (minute, hour, dom, month, dow) = (parts[0], parts[1], parts[2], parts[3], parts[4]);
    let is_num = |s: &str| s.parse::<u32>().is_ok();

    // Every N minutes: `*/N * * * *`
    if let Some(n) = minute.strip_prefix("*/")
        && hour == "*"
        && dom == "*"
        && month == "*"
        && dow == "*"
        && let Ok(n) = n.parse::<u32>()
    {
        return if n == 1 {
            "Every minute".to_string()
        } else {
            format!("Every {n} minutes")
        };
    }

    // Every hour: `M * * * *`
    if is_num(minute) && hour == "*" && dom == "*" && month == "*" && dow == "*" {
        let m: u32 = minute.parse().unwrap_or(0);
        return if m == 0 {
            "Every hour".to_string()
        } else {
            format!("Every hour at :{m:02}")
        };
    }

    // Every N hours: `M */N * * *`
    if is_num(minute)
        && let Some(n) = hour.strip_prefix("*/")
        && dom == "*"
        && month == "*"
        && dow == "*"
        && let Ok(n) = n.parse::<u32>()
    {
        let m: u32 = minute.parse().unwrap_or(0);
        let suffix = if m == 0 {
            String::new()
        } else {
            format!(" at :{m:02}")
        };
        return if n == 1 {
            format!("Every hour{suffix}")
        } else {
            format!("Every {n} hours{suffix}")
        };
    }

    // Remaining cases reference hour+minute as plain numbers.
    if !is_num(minute) || !is_num(hour) {
        return cron.to_string();
    }
    let m: u32 = minute.parse().unwrap_or(0);
    let h: u32 = hour.parse().unwrap_or(0);

    // Daily at a specific time: `M H * * *`
    if dom == "*" && month == "*" && dow == "*" {
        return format!("Every day at {}", format_local_time(m, h));
    }

    // Specific day of week: `M H * * D`
    if dom == "*"
        && month == "*"
        && dow.len() == 1
        && let Ok(d) = dow.parse::<u32>()
    {
        let day_index = (d % 7) as usize; // normalize 7 (Sunday alias) -> 0
        if let Some(day_name) = DAY_NAMES.get(day_index) {
            return format!("Every {day_name} at {}", format_local_time(m, h));
        }
    }

    // Weekdays: `M H * * 1-5`
    if dom == "*" && month == "*" && dow == "1-5" {
        return format!("Weekdays at {}", format_local_time(m, h));
    }

    cron.to_string()
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
