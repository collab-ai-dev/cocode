use super::{ApplyOutcome, apply_and_guard, apply_rtk_filter, matchable_filter};

/// A `df`-style dump with `rows` data lines. The builtin `df` filter caps output
/// at `max_lines = 20`, so any `rows > 20` produces a genuine reduction.
fn long_df_output(rows: usize) -> String {
    let mut s = String::from("Filesystem     1K-blocks   Used Available Use% Mounted on\n");
    for i in 0..rows {
        s.push_str(&format!(
            "/dev/sda{i}        4096000 123456   3972544   4% /mnt/{i}\n"
        ));
    }
    s
}

#[test]
fn matchable_filter_matches_single_command() {
    assert!(matchable_filter("df -h", "df -h").is_some());
}

#[test]
fn matchable_filter_skips_compound_and_piped_commands() {
    // A first-word (`df`) filter must not be applied to the combined output of a
    // compound or piped command — that would truncate the trailing segment.
    assert!(matchable_filter("df -h && cat config.json", "df -h && cat config.json").is_none());
    assert!(matchable_filter("df -h | tail -5", "df -h | tail -5").is_none());
}

#[test]
fn matchable_filter_misses_unknown_command() {
    assert!(
        matchable_filter(
            "this-tool-has-no-filter-xyz --flag",
            "this-tool-has-no-filter-xyz --flag"
        )
        .is_none()
    );
}

#[test]
fn matchable_filter_skips_stderr_merging_filter() {
    // `liquibase`'s builtin filter sets `filter_stderr = true`; this stdout-only
    // seam cannot honor it, so it must decline rather than half-apply.
    assert!(matchable_filter("liquibase status", "liquibase status").is_none());
}

#[test]
fn apply_and_guard_reduces_matching_command() {
    let input = long_df_output(40);
    let filter = matchable_filter("df -h", "df -h").expect("df filter matches");
    match apply_and_guard(filter, &input) {
        ApplyOutcome::Filtered(out) => {
            assert!(out.len() < input.len(), "filtered output must be smaller");
            assert!(
                !out.contains('\x1b'),
                "no ANSI escapes may leak to the model"
            );
        }
        other => panic!("expected Filtered, got {other:?}"),
    }
}

#[tokio::test]
async fn apply_rtk_filter_empty_stdout_is_none() {
    assert_eq!(apply_rtk_filter("df -h", 0, "").await, None);
}

#[tokio::test]
async fn apply_rtk_filter_rtk_disabled_prefix_skips() {
    let input = long_df_output(40);
    assert_eq!(
        apply_rtk_filter("RTK_DISABLED=1 df -h", 0, &input).await,
        None
    );
}

#[tokio::test]
async fn apply_rtk_filter_compresses_single_command() {
    let input = long_df_output(40);
    let out = apply_rtk_filter("df -h", 0, &input).await;
    assert!(
        out.expect("df dump over the line cap should compress")
            .len()
            < input.len()
    );
}

#[tokio::test]
async fn apply_rtk_filter_strips_env_prefix_before_matching() {
    // A benign leading env-var assignment must not defeat the `^df` match.
    let input = long_df_output(40);
    assert!(
        apply_rtk_filter("COLUMNS=200 df -h", 0, &input)
            .await
            .is_some()
    );
}

#[tokio::test]
async fn apply_rtk_filter_skips_compound_command() {
    // The compound guard holds end-to-end: raw output is preserved.
    let input = long_df_output(40);
    assert_eq!(
        apply_rtk_filter("df -h && echo done", 0, &input).await,
        None
    );
}
