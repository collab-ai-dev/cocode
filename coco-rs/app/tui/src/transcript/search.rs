//! Synchronous full-text matching over the reader's rendered plain-text corpus.

use crate::state::transcript::TranscriptSearchEntry;
use crate::state::transcript::TranscriptSearchLine;
use crate::state::transcript::TranscriptSearchMatch;
use coco_tui_ui::style::UiStyles;
use ratatui::buffer::Buffer;
use ratatui::buffer::CellWidth;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::Wrap;

pub(crate) fn find_matches(
    entries: &[TranscriptSearchEntry],
    query: &str,
) -> Vec<TranscriptSearchMatch> {
    if query.is_empty() {
        return Vec::new();
    }

    let mut matches = Vec::new();
    for entry in entries {
        for (line_index, line) in entry.lines.iter().enumerate() {
            for byte_range in match_ranges(&line.text, query) {
                let row_within_line = line
                    .source_rows
                    .get(byte_range.start)
                    .and_then(|row| *row)
                    .unwrap_or_default();
                matches.push(TranscriptSearchMatch {
                    cell_id: entry.cell_id.clone(),
                    line_index,
                    row_offset: line.row_offset.saturating_add(row_within_line),
                    byte_range,
                });
            }
        }
    }
    matches
}

/// Build the wrap projection once per transcript/stream/width revision. Query
/// edits then remain a cheap substring scan over cached plain text.
pub(crate) fn index_lines(lines: Vec<String>, width: u16) -> Vec<TranscriptSearchLine> {
    let mut row_offset = 0usize;
    lines
        .into_iter()
        .map(|text| {
            let source_rows = rendered_source_rows(&text, width);
            let line = TranscriptSearchLine {
                text,
                row_offset,
                source_rows,
            };
            row_offset = row_offset.saturating_add(wrapped_height(&line.text, width));
            line
        })
        .collect()
}

fn wrapped_height(text: &str, width: u16) -> usize {
    Paragraph::new(text.to_string())
        .wrap(Wrap { trim: false })
        .line_count(width.max(1))
        .max(1)
}

/// Map source UTF-8 byte offsets to the wrapped output row by aligning the
/// rendered cells with the original text. Padding cells inserted by wrapping
/// are skipped; continuation cells for wide graphemes never consume source.
fn rendered_source_rows(text: &str, width: u16) -> Vec<Option<usize>> {
    let width = width.max(1);
    let height = wrapped_height(text, width).min(u16::MAX as usize) as u16;
    let mut buffer = Buffer::empty(Rect::new(0, 0, width, height));
    Paragraph::new(text.to_string())
        .wrap(Wrap { trim: false })
        .render(buffer.area, &mut buffer);

    let mut rows = vec![None; text.len().saturating_add(1)];
    let mut source_offset = 0usize;
    for row in 0..height {
        let mut skip = 0usize;
        for col in 0..width {
            let cell = &buffer[(col, row)];
            if skip > 0 {
                skip -= 1;
                continue;
            }
            skip = usize::from(cell.cell_width().max(1)).saturating_sub(1);
            let symbol = cell.symbol();
            let Some(remaining) = text.get(source_offset..) else {
                break;
            };
            if symbol.is_empty() || !remaining.starts_with(symbol) {
                continue;
            }
            let end = source_offset.saturating_add(symbol.len()).min(text.len());
            rows[source_offset..end].fill(Some(row as usize));
            source_offset = end;
        }
    }
    if let Some(last) = rows.last_mut() {
        *last = Some(height.saturating_sub(1) as usize);
    }
    rows
}

/// Smart-case substring ranges. Queries containing an uppercase Unicode
/// scalar are exact; otherwise matching uses Unicode lowercase while mapping
/// every match back to valid byte boundaries in the original text.
pub(crate) fn match_ranges(text: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    let case_sensitive = query.chars().any(char::is_uppercase);
    if case_sensitive {
        return text
            .match_indices(query)
            .map(|(start, matched)| start..start + matched.len())
            .collect();
    }

    let needle = query.to_lowercase();
    let mut haystack = String::new();
    let mut original_ranges = Vec::new();
    for (start, ch) in text.char_indices() {
        let end = start + ch.len_utf8();
        let lowered = ch.to_lowercase().collect::<String>();
        haystack.push_str(&lowered);
        original_ranges.extend(std::iter::repeat_n(start..end, lowered.len()));
    }
    haystack
        .match_indices(&needle)
        .filter_map(|(start, matched)| {
            let end = start.checked_add(matched.len())?;
            let original_start = original_ranges.get(start)?.start;
            let original_end = original_ranges.get(end.checked_sub(1)?)?.end;
            Some(original_start..original_end)
        })
        .collect()
}

/// Project plain-text match ranges back onto styled spans without changing
/// the renderer's existing non-search styles.
pub(crate) fn apply_highlights(
    lines: &mut [Line<'static>],
    query: &str,
    current: Option<&TranscriptSearchMatch>,
    styles: UiStyles<'_>,
) {
    for (line_index, line) in lines.iter_mut().enumerate() {
        let plain: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        let ranges = match_ranges(&plain, query);
        if ranges.is_empty() {
            continue;
        }
        let current_range = current
            .filter(|current| current.line_index == line_index)
            .map(|current| &current.byte_range);
        let original = std::mem::take(&mut line.spans);
        let mut highlighted = Vec::with_capacity(original.len().saturating_add(ranges.len() * 2));
        let mut span_start = 0usize;
        for span in original {
            let content = span.content.into_owned();
            let span_end = span_start.saturating_add(content.len());
            let mut boundaries = vec![span_start, span_end];
            for range in &ranges {
                if range.start < span_end && range.end > span_start {
                    boundaries.push(range.start.max(span_start));
                    boundaries.push(range.end.min(span_end));
                }
            }
            boundaries.sort_unstable();
            boundaries.dedup();
            for pair in boundaries.windows(2) {
                let part_start = pair[0];
                let part_end = pair[1];
                let Some(part) = content.get(
                    part_start.saturating_sub(span_start)..part_end.saturating_sub(span_start),
                ) else {
                    continue;
                };
                let matching = ranges
                    .iter()
                    .find(|range| range.start < part_end && range.end > part_start);
                let style = match matching {
                    Some(range) if current_range == Some(range) => span.style.patch(
                        Style::default()
                            .fg(styles.selection_fg())
                            .bg(styles.selection_bg())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Some(_) => span.style.patch(
                        Style::default()
                            .fg(styles.search_match())
                            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                    ),
                    None => span.style,
                };
                highlighted.push(Span::styled(part.to_string(), style));
            }
            span_start = span_end;
        }
        line.spans = highlighted;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smart_case_ranges_preserve_original_byte_offsets() {
        assert_eq!(match_ranges("Alpha alpha", "alpha"), vec![0..5, 6..11]);
        assert_eq!(match_ranges("Alpha alpha", "Alpha"), vec![0..5]);
        assert_eq!(match_ranges("你好 alpha", "alpha"), vec![7..12]);
        assert_eq!(match_ranges("RÉSUMÉ résumé", "résumé"), vec![0..8, 9..17]);
        assert!(match_ranges("RÉSUMÉ résumé", "Résumé").is_empty());
    }

    #[test]
    fn indexed_lines_cache_wrapped_rows_for_query_time_navigation() {
        let entry = TranscriptSearchEntry {
            cell_id: crate::state::transcript::TranscriptCellId::message(0, "m1"),
            lines: index_lines(vec!["alpha bravo cobra needle".to_string()], 8),
        };

        let matches = find_matches(&[entry], "needle");

        assert_eq!(matches.len(), 1);
        assert!(matches[0].row_offset > 0);
    }

    #[test]
    fn highlights_current_and_other_matches_across_styled_span_boundaries() {
        let theme = crate::theme::Theme::default();
        let styles = UiStyles::new(&theme);
        let mut lines = vec![Line::from(vec![Span::raw("Al"), Span::raw("pha alpha")])];
        let current = TranscriptSearchMatch {
            cell_id: crate::state::transcript::TranscriptCellId::message(0, "m1"),
            line_index: 0,
            row_offset: 0,
            byte_range: 0..5,
        };

        apply_highlights(&mut lines, "alpha", Some(&current), styles);

        let selected = lines[0]
            .spans
            .iter()
            .filter(|span| span.content.as_ref() == "Al" || span.content.as_ref() == "pha")
            .collect::<Vec<_>>();
        assert_eq!(selected.len(), 2);
        assert!(
            selected
                .iter()
                .all(|span| span.style.bg == Some(styles.selection_bg()))
        );
        let other = lines[0]
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "alpha")
            .expect("second match");
        assert!(
            other
                .style
                .add_modifier
                .contains(ratatui::style::Modifier::UNDERLINED)
        );
    }

    #[test]
    fn unicode_match_maps_through_wide_graphemes_and_wrapping() {
        let entry = TranscriptSearchEntry {
            cell_id: crate::state::transcript::TranscriptCellId::message(0, "m1"),
            lines: index_lines(vec!["界界🙂 needle".to_string()], 4),
        };

        let matches = find_matches(&[entry], "needle");

        assert_eq!(matches.len(), 1);
        assert!(matches[0].row_offset >= 2);
    }

    #[test]
    fn unicode_highlight_crosses_styled_span_boundaries() {
        let theme = crate::theme::Theme::default();
        let styles = UiStyles::new(&theme);
        let mut lines = vec![Line::from(vec![
            Span::raw("你"),
            Span::raw("好🙂"),
            Span::raw("世界"),
        ])];
        let range = match_ranges("你好🙂世界", "好🙂世")
            .into_iter()
            .next()
            .expect("unicode match");
        let current = TranscriptSearchMatch {
            cell_id: crate::state::transcript::TranscriptCellId::message(0, "m1"),
            line_index: 0,
            row_offset: 0,
            byte_range: range,
        };

        apply_highlights(&mut lines, "好🙂世", Some(&current), styles);

        let selected = lines[0]
            .spans
            .iter()
            .filter(|span| span.style.bg == Some(styles.selection_bg()))
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert_eq!(selected, "好🙂世");
    }
}
