use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn test_normalize_timestamps_replaces_rfc3339_variants() {
    let input = "started 2025-01-15T10:00:00Z ended 2025-01-15T10:00:01.250+02:00";
    assert_eq!(
        normalize_timestamps(input),
        "started <TIMESTAMP> ended <TIMESTAMP>"
    );
}

#[test]
fn test_normalize_timestamps_leaves_plain_dates_untouched() {
    // A bare date (no time component) is not a volatile wall-clock stamp.
    let input = "2025-01-15 is a date";
    assert_eq!(normalize_timestamps(input), input);
}

#[test]
fn test_normalize_uuids_replaces_v4_uuid() {
    let input = "session 67e55044-10b1-426f-9247-bb680e5fe0c8 done";
    assert_eq!(normalize_uuids(input), "session <UUID> done");
}

#[test]
fn test_normalize_temp_paths_collapses_tempfile_component() {
    // Independent of the real temp dir: the random `.tmpXXXX` component alone
    // must collapse.
    let input = "wrote /some/dir/.tmpAb3Xz9/out.txt";
    assert_eq!(normalize_temp_paths(input), "wrote /some/dir/<TMP>/out.txt");
}

#[test]
fn test_normalize_temp_paths_collapses_system_temp_dir() {
    let tmp = std::env::temp_dir();
    let path = tmp.join("golden_fixture.txt");
    let rendered = format!("path={}", path.display());
    let normalized = normalize_temp_paths(&rendered);
    assert_eq!(normalized, "path=<TMP>/golden_fixture.txt");
}

#[test]
fn test_normalize_str_applies_all_in_sequence() {
    let input = "turn 67e55044-10b1-426f-9247-bb680e5fe0c8 at 2025-01-15T10:00:00Z in /x/.tmpQ9/f";
    assert_eq!(
        normalize_str(input),
        "turn <UUID> at <TIMESTAMP> in /x/<TMP>/f"
    );
}

#[test]
fn test_normalize_json_value_walks_nested_strings_keeps_keys_and_scalars() {
    let mut value = json!({
        "session_id": "67e55044-10b1-426f-9247-bb680e5fe0c8",
        "timestamp": "2025-01-15T10:00:00Z",
        "count": 3,
        "active": true,
        "nested": [
            { "at": "2024-12-31T23:59:59Z" },
            "no-volatile-data"
        ]
    });
    normalize_json_value(&mut value);
    assert_eq!(
        value,
        json!({
            "session_id": "<UUID>",
            "timestamp": "<TIMESTAMP>",
            "count": 3,
            "active": true,
            "nested": [
                { "at": "<TIMESTAMP>" },
                "no-volatile-data"
            ]
        })
    );
}
