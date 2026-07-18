//! Fuzzy matching over the composer's prompt history for the Ctrl+R overlay.
//!
//! Mirrors [`super::skill_search`]: a synchronous, in-memory nucleo fuzzy match
//! that ranks entries and reports per-character highlight indices, so the Ctrl+R
//! search shows a visible ranked list instead of stepping one substring hit at a
//! time. Fed by the same in-memory `input.history` the reverse-search already
//! reads (index 0 = newest).

use nucleo::Matcher;
use nucleo::Utf32String;
use nucleo::pattern::AtomKind;
use nucleo::pattern::CaseMatching;
use nucleo::pattern::Normalization;
use nucleo::pattern::Pattern;

use crate::state::HistoryEntry;
use crate::widgets::suggestion_popup::SuggestionItem;

/// Max history matches shown in the Ctrl+R overlay.
pub(crate) const MAX_HISTORY_RESULTS: usize = 8;

/// Ranked history results: popup rows plus the `input.history` index each row
/// points at (parallel vectors, same length).
#[derive(Debug, Default, Clone)]
pub(crate) struct HistoryResults {
    pub(crate) items: Vec<SuggestionItem>,
    pub(crate) indices: Vec<usize>,
}

/// Fuzzy-rank `history` (index 0 = newest) against `query`.
///
/// An empty query browses the newest entries in recency order (no highlights).
/// Otherwise nucleo scores every entry; ties break toward the newer entry. Each
/// row's label is the entry's first line (a `…` marks a multi-line prompt).
pub(crate) fn search_history(history: &[HistoryEntry], query: &str) -> HistoryResults {
    if query.is_empty() {
        let mut results = HistoryResults::default();
        for (idx, entry) in history.iter().take(MAX_HISTORY_RESULTS).enumerate() {
            results.items.push(to_item(entry, Vec::new()));
            results.indices.push(idx);
        }
        return results;
    }

    let pattern = Pattern::new(
        query,
        CaseMatching::Ignore,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );
    let mut matcher = Matcher::default();
    let mut scored: Vec<(u32, usize)> = Vec::new();
    for (idx, entry) in history.iter().enumerate() {
        // The popup displays the first line only. Match that same text so a
        // row is never selected for an invisible hit on a later line.
        let haystack = Utf32String::from(entry.text.lines().next().unwrap_or(""));
        if let Some(score) = pattern.score(haystack.slice(..), &mut matcher)
            && score > 0
        {
            scored.push((score, idx));
        }
    }
    // Highest score first; newer (lower index) first on ties.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));

    let mut results = HistoryResults::default();
    for (_, idx) in scored.into_iter().take(MAX_HISTORY_RESULTS) {
        let entry = &history[idx];
        let text = &entry.text;
        results
            .items
            .push(to_item(entry, match_indices(&pattern, &mut matcher, text)));
        results.indices.push(idx);
    }
    results
}

/// Char positions of the query hit against the label's first line (the label is
/// the first line; a hit needing later lines has nothing to mark there).
fn match_indices(pattern: &Pattern, matcher: &mut Matcher, text: &str) -> Vec<i32> {
    let first = text.lines().next().unwrap_or("");
    let haystack = Utf32String::from(first);
    let mut indices = Vec::<u32>::new();
    if pattern
        .indices(haystack.slice(..), matcher, &mut indices)
        .is_none()
    {
        return Vec::new();
    }
    indices.sort_unstable();
    indices.dedup();
    indices
        .into_iter()
        .filter_map(|i| i32::try_from(i).ok())
        .collect()
}

fn to_item(entry: &HistoryEntry, highlight_indices: Vec<i32>) -> SuggestionItem {
    let text = &entry.text;
    let first = text.lines().next().unwrap_or("");
    let label = if text.contains('\n') {
        format!("{first} …")
    } else {
        first.to_string()
    };
    SuggestionItem {
        highlight_indices,
        label,
        description: entry.timestamp_ms.map(relative_timestamp),
        metadata: None,
    }
}

fn relative_timestamp(timestamp_ms: i64) -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64;
    let age_seconds = now_ms.saturating_sub(timestamp_ms).max(0) / 1_000;
    match age_seconds {
        0..=59 => "now".to_string(),
        60..=3_599 => format!("{}m ago", age_seconds / 60),
        3_600..=86_399 => format!("{}h ago", age_seconds / 3_600),
        _ => format!("{}d ago", age_seconds / 86_400),
    }
}

#[cfg(test)]
#[path = "history_search.test.rs"]
mod tests;
