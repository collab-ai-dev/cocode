use super::*;

#[test]
fn pages_items_with_numeric_cursor_and_limit() {
    let page = session_data_page(5, 2, Some(2));
    assert_eq!(page.start, 2);
    assert_eq!(page.end, 4);
    assert_eq!(page.next_cursor().as_deref(), Some("4"));
    assert!(page.has_more);

    let (items, next_cursor, has_more) = page_session_items(&[1, 2, 3, 4, 5], 4, Some(4));
    assert_eq!(items, vec![5]);
    assert_eq!(next_cursor, None);
    assert!(!has_more);
}

#[test]
fn rejects_invalid_cursor_and_limit() {
    let cursor =
        parse_session_data_cursor("session/read", Some("bad")).expect_err("invalid cursor");
    assert_eq!(cursor.message(), "session/read: invalid cursor \"bad\"");

    let limit = parse_session_data_limit("session/read", Some(-1)).expect_err("invalid limit");
    assert_eq!(limit.message(), "session/read: invalid limit -1");
}

#[test]
fn derives_turn_spans_from_user_boundaries() {
    let turns = derive_session_turn_summaries([
        TranscriptTurnEntry {
            is_user: true,
            timestamp: Some("2026-01-01T00:00:00Z"),
        },
        TranscriptTurnEntry {
            is_user: false,
            timestamp: Some("2026-01-01T00:00:01Z"),
        },
        TranscriptTurnEntry {
            is_user: true,
            timestamp: Some("2026-01-01T00:00:02Z"),
        },
    ]);

    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0].index, 0);
    assert_eq!(turns[0].start_cursor, "0");
    assert_eq!(turns[0].message_count, 2);
    assert_eq!(turns[0].ended_at.as_deref(), Some("2026-01-01T00:00:01Z"));
    assert_eq!(turns[1].index, 1);
    assert_eq!(turns[1].start_cursor, "2");
    assert_eq!(turns[1].message_count, 1);
}
