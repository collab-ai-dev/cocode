//! Shared relative-time formatting for transcript and session pickers.

/// Format an epoch-ms timestamp against a reference `now` (also epoch-ms).
pub(crate) fn format_age(now_ms: i64, then_ms: i64) -> String {
    let delta_secs = ((now_ms - then_ms).max(0) / 1000) as u64;
    if delta_secs < 60 {
        return "just now".to_string();
    }
    if delta_secs < 60 * 60 {
        let minutes = delta_secs / 60;
        return if minutes == 1 {
            "1 minute ago".to_string()
        } else {
            format!("{minutes} minutes ago")
        };
    }
    if delta_secs < 60 * 60 * 24 {
        let hours = delta_secs / 3600;
        return if hours == 1 {
            "1 hour ago".to_string()
        } else {
            format!("{hours} hours ago")
        };
    }
    let days = delta_secs / (60 * 60 * 24);
    if days == 1 {
        "1 day ago".to_string()
    } else {
        format!("{days} days ago")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_relative_age() {
        let now = 200_000_000;
        assert_eq!(format_age(now, now), "just now");
        assert_eq!(format_age(now, now - 120_000), "2 minutes ago");
        assert_eq!(format_age(now, now - 7_200_000), "2 hours ago");
        assert_eq!(format_age(now, now - 172_800_000), "2 days ago");
    }
}
