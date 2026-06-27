//! Pure view model for unified diff rendering.
//!
//! The widget layer owns ratatui spans and theme colors. This module owns
//! parsing, row classification, and old/new line-number progression so chat
//! cells and full-screen diff modals share one source-backed model.

/// Parsed line numbers from a unified diff hunk header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HunkHeader {
    pub old_start: i32,
    pub new_start: i32,
    pub label: String,
}

/// A single source-backed diff row before styling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffLineView {
    FileHeader {
        marker: &'static str,
        path: String,
    },
    Hunk {
        old_start: i32,
        new_start: i32,
        label: String,
    },
    RawHunk {
        text: String,
    },
    Context {
        old_line: i32,
        new_line: i32,
        content: String,
    },
    Removed {
        old_line: i32,
        content: String,
        compare_to: Option<String>,
    },
    Added {
        new_line: i32,
        content: String,
        compare_to: Option<String>,
    },
}

/// Borrowed counterpart to [`DiffLineView`] for internal render paths that
/// should avoid cloning large diff bodies before styling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiffLineViewRef<'a> {
    FileHeader {
        marker: &'static str,
        path: &'a str,
    },
    Hunk {
        old_start: i32,
        new_start: i32,
        label: &'a str,
    },
    RawHunk {
        text: &'a str,
    },
    Context {
        old_line: i32,
        new_line: i32,
        content: &'a str,
    },
    Removed {
        old_line: i32,
        content: &'a str,
        compare_to: Option<&'a str>,
    },
    Added {
        new_line: i32,
        content: &'a str,
        compare_to: Option<&'a str>,
    },
}

/// A bounded head/tail view of a diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiffLineViewWindow<'a> {
    pub(crate) head: Vec<DiffLineViewRef<'a>>,
    pub(crate) tail: Vec<DiffLineViewRef<'a>>,
    pub(crate) omitted: usize,
}

struct HunkHeaderRef<'a> {
    old_start: i32,
    new_start: i32,
    label: &'a str,
}

/// Parse a unified diff `@@` line into old/new start positions.
pub fn parse_hunk_header(line: &str) -> Option<HunkHeader> {
    let hdr = parse_hunk_header_ref(line)?;
    Some(HunkHeader {
        old_start: hdr.old_start,
        new_start: hdr.new_start,
        label: hdr.label.to_string(),
    })
}

fn parse_hunk_header_ref(line: &str) -> Option<HunkHeaderRef<'_>> {
    // Format: @@ -old_start[,old_count] +new_start[,new_count] @@ [label]
    let stripped = line.strip_prefix("@@ ")?;
    let end_idx = stripped.find(" @@")?;
    let range_part = &stripped[..end_idx];
    let label = stripped.get(end_idx + 3..).unwrap_or("").trim();

    let mut parts = range_part.split_whitespace();

    let old_range = parts.next()?.strip_prefix('-')?;
    let old_start: i32 = old_range
        .split(',')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);

    let new_range = parts.next()?.strip_prefix('+')?;
    let new_start: i32 = new_range
        .split(',')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);

    Some(HunkHeaderRef {
        old_start,
        new_start,
        label,
    })
}

pub fn diff_line_views(diff_text: &str) -> Vec<DiffLineView> {
    diff_line_view_refs(diff_text)
        .into_iter()
        .map(DiffLineView::from)
        .collect()
}

pub(crate) fn diff_line_view_refs<'a>(diff_text: &'a str) -> Vec<DiffLineViewRef<'a>> {
    let raw_lines: Vec<&str> = diff_text.lines().collect();
    let mut rows = Vec::new();
    emit_diff_line_views(&raw_lines, |row| rows.push(row));
    rows
}

/// Collect at most `row_limit` logical diff rows using a head/tail split.
///
/// The scan is still linear so line numbers remain correct, but storage is
/// bounded and callers can render only the retained rows.
pub(crate) fn diff_line_view_window<'a>(
    diff_text: &'a str,
    row_limit: usize,
) -> DiffLineViewWindow<'a> {
    let raw_lines: Vec<&str> = diff_text.lines().collect();
    let head_limit = row_limit.div_ceil(2);
    let tail_limit = row_limit / 2;
    let mut head = Vec::with_capacity(head_limit);
    let mut tail = std::collections::VecDeque::with_capacity(tail_limit);
    let mut total = 0usize;

    emit_diff_line_views(&raw_lines, |row| {
        if total < head_limit {
            head.push(row);
        } else if tail_limit > 0 {
            if tail.len() == tail_limit {
                tail.pop_front();
            }
            tail.push_back(row);
        }
        total += 1;
    });

    let tail: Vec<_> = tail.into_iter().collect();
    let omitted = total.saturating_sub(head.len() + tail.len());
    DiffLineViewWindow {
        head,
        tail,
        omitted,
    }
}

fn emit_diff_line_views<'a>(lines: &[&'a str], mut push: impl FnMut(DiffLineViewRef<'a>)) {
    let mut old_line: i32 = 1;
    let mut new_line: i32 = 1;
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];

        if line.starts_with("--- ") || line.starts_with("+++ ") {
            let (marker, path) = if let Some(rest) = line.strip_prefix("--- ") {
                ("─", rest)
            } else if let Some(rest) = line.strip_prefix("+++ ") {
                ("+", rest)
            } else {
                ("", line)
            };
            push(DiffLineViewRef::FileHeader { marker, path });
            i += 1;
        } else if line.starts_with("@@") {
            if let Some(hdr) = parse_hunk_header_ref(line) {
                old_line = hdr.old_start;
                new_line = hdr.new_start;
                push(DiffLineViewRef::Hunk {
                    old_start: hdr.old_start,
                    new_start: hdr.new_start,
                    label: hdr.label,
                });
            } else {
                push(DiffLineViewRef::RawHunk { text: line });
            }
            i += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            let rm_start = i;
            while i < lines.len() && lines[i].starts_with('-') && !lines[i].starts_with("---") {
                i += 1;
            }
            let add_start = i;
            while i < lines.len() && lines[i].starts_with('+') && !lines[i].starts_with("+++") {
                i += 1;
            }
            let removed = &lines[rm_start..add_start];
            let added = &lines[add_start..i];

            let pairs = removed.len().min(added.len());
            for j in 0..pairs {
                let old_content = removed[j].strip_prefix('-').unwrap_or(removed[j]);
                let new_content = added[j].strip_prefix('+').unwrap_or(added[j]);
                push(DiffLineViewRef::Removed {
                    old_line,
                    content: old_content,
                    compare_to: Some(new_content),
                });
                old_line += 1;
                push(DiffLineViewRef::Added {
                    new_line,
                    content: new_content,
                    compare_to: Some(old_content),
                });
                new_line += 1;
            }
            for line in &removed[pairs..] {
                push(DiffLineViewRef::Removed {
                    old_line,
                    content: line.strip_prefix('-').unwrap_or(line),
                    compare_to: None,
                });
                old_line += 1;
            }
            for line in &added[pairs..] {
                push(DiffLineViewRef::Added {
                    new_line,
                    content: line.strip_prefix('+').unwrap_or(line),
                    compare_to: None,
                });
                new_line += 1;
            }
        } else if line.starts_with('+') && !line.starts_with("+++") {
            push(DiffLineViewRef::Added {
                new_line,
                content: line.strip_prefix('+').unwrap_or(line),
                compare_to: None,
            });
            new_line += 1;
            i += 1;
        } else {
            push(DiffLineViewRef::Context {
                old_line,
                new_line,
                content: line.strip_prefix(' ').unwrap_or(line),
            });
            old_line += 1;
            new_line += 1;
            i += 1;
        }
    }
}

impl From<DiffLineViewRef<'_>> for DiffLineView {
    fn from(row: DiffLineViewRef<'_>) -> Self {
        match row {
            DiffLineViewRef::FileHeader { marker, path } => Self::FileHeader {
                marker,
                path: path.to_string(),
            },
            DiffLineViewRef::Hunk {
                old_start,
                new_start,
                label,
            } => Self::Hunk {
                old_start,
                new_start,
                label: label.to_string(),
            },
            DiffLineViewRef::RawHunk { text } => Self::RawHunk {
                text: text.to_string(),
            },
            DiffLineViewRef::Context {
                old_line,
                new_line,
                content,
            } => Self::Context {
                old_line,
                new_line,
                content: content.to_string(),
            },
            DiffLineViewRef::Removed {
                old_line,
                content,
                compare_to,
            } => Self::Removed {
                old_line,
                content: content.to_string(),
                compare_to: compare_to.map(str::to_string),
            },
            DiffLineViewRef::Added {
                new_line,
                content,
                compare_to,
            } => Self::Added {
                new_line,
                content: content.to_string(),
                compare_to: compare_to.map(str::to_string),
            },
        }
    }
}

#[cfg(test)]
#[path = "diff.test.rs"]
mod tests;
