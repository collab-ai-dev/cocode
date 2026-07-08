use coco_types::SdkSessionTurnSummary;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionDataProjectionError {
    InvalidCursor { operation: String, raw: String },
    InvalidLimit { operation: String, limit: i32 },
}

impl SessionDataProjectionError {
    pub fn message(&self) -> String {
        match self {
            Self::InvalidCursor { operation, raw } => {
                format!("{operation}: invalid cursor {raw:?}")
            }
            Self::InvalidLimit { operation, limit } => {
                format!("{operation}: invalid limit {limit}")
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionPage {
    pub start: usize,
    pub end: usize,
    pub has_more: bool,
}

impl SessionPage {
    pub fn next_cursor(self) -> Option<String> {
        self.has_more.then(|| self.end.to_string())
    }
}

pub fn parse_session_data_cursor(
    operation: &str,
    raw: Option<&str>,
) -> Result<usize, SessionDataProjectionError> {
    match raw {
        Some(raw) => raw
            .parse::<usize>()
            .map_err(|_| SessionDataProjectionError::InvalidCursor {
                operation: operation.to_string(),
                raw: raw.to_string(),
            }),
        None => Ok(0),
    }
}

pub fn parse_session_data_limit(
    operation: &str,
    limit: Option<i32>,
) -> Result<Option<usize>, SessionDataProjectionError> {
    match limit {
        Some(limit) if limit < 0 => Err(SessionDataProjectionError::InvalidLimit {
            operation: operation.to_string(),
            limit,
        }),
        Some(limit) => Ok(Some(limit as usize)),
        None => Ok(None),
    }
}

pub fn session_data_page(total: usize, cursor: usize, limit: Option<usize>) -> SessionPage {
    let start = cursor.min(total);
    let end = match limit {
        Some(limit) => start.saturating_add(limit).min(total),
        None => total,
    };
    SessionPage {
        start,
        end,
        has_more: end < total,
    }
}

pub fn page_session_items<T: Clone>(
    items: &[T],
    cursor: usize,
    limit: Option<usize>,
) -> (Vec<T>, Option<String>, bool) {
    let page = session_data_page(items.len(), cursor, limit);
    (
        items[page.start..page.end].to_vec(),
        page.next_cursor(),
        page.has_more,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptTurnEntry<'a> {
    pub is_user: bool,
    pub timestamp: Option<&'a str>,
}

pub fn derive_session_turn_summaries<'a>(
    entries: impl IntoIterator<Item = TranscriptTurnEntry<'a>>,
) -> Vec<SdkSessionTurnSummary> {
    let mut spans = Vec::new();
    let mut current: Option<TurnSpanBuilder> = None;
    for (message_index, entry) in entries.into_iter().enumerate() {
        if entry.is_user || current.is_none() {
            if let Some(span) = current.take() {
                spans.push(span.finish(spans.len()));
            }
            current = Some(TurnSpanBuilder::new(message_index, entry.timestamp));
        } else if let Some(span) = current.as_mut() {
            span.message_count += 1;
            span.ended_at = entry.timestamp.and_then(non_empty_timestamp);
        }
    }
    if let Some(span) = current {
        spans.push(span.finish(spans.len()));
    }
    spans
}

#[derive(Debug, Clone)]
struct TurnSpanBuilder {
    start_message_index: usize,
    message_count: i32,
    started_at: Option<String>,
    ended_at: Option<String>,
}

impl TurnSpanBuilder {
    fn new(start_message_index: usize, timestamp: Option<&str>) -> Self {
        let timestamp = timestamp.and_then(non_empty_timestamp);
        Self {
            start_message_index,
            message_count: 1,
            started_at: timestamp.clone(),
            ended_at: timestamp,
        }
    }

    fn finish(self, index: usize) -> SdkSessionTurnSummary {
        SdkSessionTurnSummary {
            index: index as i32,
            start_cursor: self.start_message_index.to_string(),
            message_count: self.message_count,
            started_at: self.started_at,
            ended_at: self.ended_at,
        }
    }
}

fn non_empty_timestamp(timestamp: &str) -> Option<String> {
    (!timestamp.is_empty()).then(|| timestamp.to_string())
}

#[cfg(test)]
mod tests {
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
}
