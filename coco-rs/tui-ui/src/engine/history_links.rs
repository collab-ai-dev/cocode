//! Hyperlink detection and post-wrap geometry for finalized history rows.

use std::path::Path;

use ratatui::buffer::Buffer;
use ratatui::buffer::CellWidth;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;
use url::Url;

/// A clickable run in a finalized, already-wrapped history row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryLinkRun {
    pub row: u16,
    pub start_col: u16,
    pub end_col: u16,
    pub target: String,
}

/// Link geometry on one logical, pre-wrap line. Markdown renderers produce
/// this sidecar without embedding terminal escapes into visible span text;
/// history insertion resolves it to final row/column runs after wrapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryLinkHint {
    pub line: usize,
    pub start_byte: usize,
    pub end_byte: usize,
    pub target: String,
}

pub(super) fn resolve_links(
    pending: Vec<PendingLineLinks>,
    buffer: &Buffer,
    width: u16,
) -> Vec<HistoryLinkRun> {
    pending
        .into_iter()
        .flat_map(|pending| pending.resolve(buffer, width))
        .collect()
}

#[derive(Debug)]
pub(super) struct PendingLineLinks {
    first_row: u16,
    row_count: u16,
    source_text: String,
    links: Vec<DetectedLink>,
}

impl PendingLineLinks {
    fn resolve(self, buffer: &Buffer, width: u16) -> Vec<HistoryLinkRun> {
        if self.links.is_empty() || self.row_count == 0 {
            return Vec::new();
        }

        let mut cells = rendered_text_cells(
            buffer,
            self.first_row..self.first_row.saturating_add(self.row_count),
            width,
        );
        align_cells_to_source(&mut cells, &self.source_text);
        let mut runs: Vec<HistoryLinkRun> = Vec::new();
        for link in self.links {
            for cell in cells.iter().filter(|cell| {
                cell.source_start < link.source_end && cell.source_end > link.source_start
            }) {
                if let Some(run) = runs.last_mut()
                    && run.row == cell.row
                    && run.end_col == cell.col
                    && run.target == link.target
                {
                    run.end_col = cell.end_col;
                } else {
                    runs.push(HistoryLinkRun {
                        row: cell.row,
                        start_col: cell.col,
                        end_col: cell.end_col,
                        target: link.target.clone(),
                    });
                }
            }
        }
        runs
    }
}

#[derive(Debug)]
struct RenderedTextCell {
    row: u16,
    col: u16,
    end_col: u16,
    source_start: usize,
    source_end: usize,
    text: String,
}

/// Map post-wrap cells back to their byte ranges in the logical source line.
/// Ratatui's word wrapper may omit whitespace at a row boundary, so rendered
/// byte offsets are not interchangeable with source byte offsets. The visible
/// cells otherwise remain an ordered subsequence of the source; aligning them
/// once preserves the exact Markdown link occurrence even when an identical
/// unlinked label appears earlier on the line.
fn align_cells_to_source(cells: &mut [RenderedTextCell], source: &str) {
    let mut search_from = 0usize;
    for cell in cells {
        let Some(relative_start) = source[search_from..].find(&cell.text) else {
            cell.source_start = source.len();
            cell.source_end = source.len();
            continue;
        };
        cell.source_start = search_from + relative_start;
        cell.source_end = cell.source_start + cell.text.len();
        search_from = cell.source_end;
    }
}

fn rendered_text_cells(
    buffer: &Buffer,
    rows: std::ops::Range<u16>,
    width: u16,
) -> Vec<RenderedTextCell> {
    let mut cells = Vec::new();
    for row in rows {
        let mut row_cells = Vec::new();
        let mut skip = 0usize;
        for col in 0..width {
            let cell = &buffer[(col, row)];
            if skip == 0 {
                let cell_width = cell.cell_width().max(1);
                row_cells.push((col, (col + cell_width).min(width), cell.symbol()));
                skip = usize::from(cell_width).saturating_sub(1);
            } else {
                skip -= 1;
            }
        }
        while row_cells
            .last()
            .is_some_and(|(_, _, symbol)| symbol.chars().all(char::is_whitespace))
        {
            row_cells.pop();
        }
        for (col, end_col, symbol) in row_cells {
            cells.push(RenderedTextCell {
                row,
                col,
                end_col,
                source_start: 0,
                source_end: 0,
                text: symbol.to_string(),
            });
        }
    }
    cells
}

pub(super) fn pending_links(
    lines: &[Line<'static>],
    width: u16,
    base_dir: Option<&Path>,
    explicit: &[HistoryLinkHint],
) -> Vec<PendingLineLinks> {
    let mut first_row = 0u16;
    lines
        .iter()
        .enumerate()
        .map(|(line_index, line)| {
            let row_count = Paragraph::new(line.clone())
                .wrap(Wrap { trim: false })
                .line_count(width)
                .min(u16::MAX as usize) as u16;
            let text = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>();
            let explicit = explicit
                .iter()
                .filter(|link| link.line == line_index)
                .collect::<Vec<_>>();
            let mut links = detect_links(&text, base_dir);
            links.retain(|detected| {
                !explicit.iter().any(|link| {
                    link.start_byte < detected.source_end && link.end_byte > detected.source_start
                })
            });
            links.extend(explicit.into_iter().filter_map(|link| {
                text.get(link.start_byte..link.end_byte)?;
                Some(DetectedLink {
                    source_start: link.start_byte,
                    source_end: link.end_byte,
                    target: link.target.clone(),
                })
            }));
            links.sort_by_key(|link| (link.source_start, link.source_end));
            let pending = PendingLineLinks {
                first_row,
                row_count,
                source_text: text,
                links,
            };
            first_row = first_row.saturating_add(row_count);
            pending
        })
        .collect()
}

#[derive(Debug)]
struct DetectedLink {
    source_start: usize,
    source_end: usize,
    target: String,
}

fn detect_links(text: &str, base_dir: Option<&Path>) -> Vec<DetectedLink> {
    let mut links = linkify::LinkFinder::new()
        .links(text)
        .filter_map(|link| {
            let display = link.as_str();
            let target = match link.kind() {
                linkify::LinkKind::Url => normalize_url(display)?,
                linkify::LinkKind::Email => format!("mailto:{display}"),
                _ => return None,
            };
            Some(DetectedLink {
                source_start: link.start(),
                source_end: link.end(),
                target,
            })
        })
        .collect::<Vec<_>>();

    for (start, end) in whitespace_delimited_ranges(text) {
        let (start, end) = trim_path_punctuation(text, start, end);
        if start == end
            || links
                .iter()
                .any(|link| start < link.source_end && end > link.source_start)
        {
            continue;
        }
        let display = &text[start..end];
        let Some(target) = file_link_target(display, base_dir) else {
            continue;
        };
        links.push(DetectedLink {
            source_start: start,
            source_end: end,
            target,
        });
    }
    links.sort_by_key(|link| link.source_start);
    links
}

fn normalize_url(display: &str) -> Option<String> {
    let candidate = if display.starts_with("www.") {
        format!("https://{display}")
    } else {
        display.to_string()
    };
    let url = Url::parse(&candidate).ok()?;
    matches!(url.scheme(), "http" | "https" | "mailto" | "file").then_some(candidate)
}

fn whitespace_delimited_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = None;
    for (index, ch) in text.char_indices() {
        if ch.is_whitespace() {
            if let Some(start) = start.take() {
                ranges.push((start, index));
            }
        } else if start.is_none() {
            start = Some(index);
        }
    }
    if let Some(start) = start {
        ranges.push((start, text.len()));
    }
    ranges
}

fn trim_path_punctuation(text: &str, mut start: usize, mut end: usize) -> (usize, usize) {
    while start < end {
        let Some(ch) = text[start..end].chars().next() else {
            break;
        };
        if matches!(ch, '(' | '[' | '{' | '<' | '\'' | '"') {
            start += ch.len_utf8();
        } else {
            break;
        }
    }
    while start < end {
        let Some(ch) = text[start..end].chars().next_back() else {
            break;
        };
        if matches!(
            ch,
            ')' | ']' | '}' | '>' | '\'' | '"' | ',' | ';' | '!' | '?'
        ) {
            end -= ch.len_utf8();
        } else {
            break;
        }
    }
    (start, end)
}

fn file_link_target(display: &str, base_dir: Option<&Path>) -> Option<String> {
    let path_text = strip_file_location(display);
    let path = Path::new(path_text);
    let plausible = if path.is_absolute() {
        path_text[1..].contains('/') || path_text.contains('.')
    } else {
        path_text.starts_with("./") || path_text.starts_with("../") || is_bare_repository_file(path)
    };
    if !plausible || path_text.contains("://") {
        return None;
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir?.join(path)
    };
    Url::from_file_path(absolute).ok().map(Into::into)
}

fn is_bare_repository_file(path: &Path) -> bool {
    let Some(path_text) = path.to_str() else {
        return false;
    };
    if !path_text.contains('/') || path_text.ends_with('/') {
        return false;
    }
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    path.extension().is_some()
        || (file_name.starts_with('.') && file_name.len() > 1)
        || matches!(
            file_name,
            "Makefile" | "Dockerfile" | "Justfile" | "LICENSE" | "README" | "CHANGELOG"
        )
}

/// Keep editor-style locations clickable as a whole while making the OSC 8
/// target a valid file URL. Both `path:line` and `path:line:column` are
/// accepted; a colon that is not followed by decimal digits remains part of
/// the path.
fn strip_file_location(display: &str) -> &str {
    let mut path_end = display.len();
    for _ in 0..2 {
        let candidate = &display[..path_end];
        let Some((path, suffix)) = candidate.rsplit_once(':') else {
            break;
        };
        if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
            break;
        }
        path_end = path.len();
    }
    &display[..path_end]
}
