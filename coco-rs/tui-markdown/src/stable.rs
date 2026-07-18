use std::collections::HashSet;

/// Incremental conservative Markdown stable-boundary finder.
///
/// [`push`](Self::push) accepts only newly appended source bytes. Complete
/// lines are scanned once; the unfinished line is retained until a later push.
/// Returning too little only keeps more text in the mutable tail, while
/// returning too much could commit rows whose interpretation later changes.
#[derive(Debug, Default, Clone)]
pub struct StablePrefixTracker {
    processed_len: usize,
    pending_line: String,
    safe_end: usize,
    fence_open: Option<FenceMarker>,
    html_block_end: Option<HtmlBlockEnd>,
    in_indented_code_tail: bool,
    // A trailing list that can still grow is held back entirely: a later
    // sibling item separated by a blank line flips the WHOLE list from tight
    // to loose (CommonMark), retroactively rewriting items that were already
    // rendered. `list_guard` remembers the last safe boundary before the open
    // list began; it caps the result only while the list can still continue
    // past the end of the scanned source.
    in_list_tail: bool,
    list_guard: usize,
    prev_blank: bool,
    reference_definitions: HashSet<String>,
    unresolved_references: HashSet<String>,
    pending_reference_candidate: String,
    has_reference_definitions: bool,
}

impl StablePrefixTracker {
    /// Scan appended source and return the current absolute stable-prefix end.
    pub fn push(&mut self, appended: &str) -> usize {
        self.pending_line.push_str(appended);
        let Some(scan_end) = self.pending_line.rfind('\n').map(|idx| idx + 1) else {
            return self.stable_end();
        };

        let mut complete = std::mem::take(&mut self.pending_line);
        self.pending_line = complete.split_off(scan_end);
        for line in complete.split_inclusive('\n') {
            self.process_complete_line(line);
        }
        self.stable_end()
    }

    /// Whether independently rendering a later source slice could lose global
    /// reference-link definitions established by an earlier slice.
    pub fn requires_document_context(&self) -> bool {
        self.has_reference_definitions
    }

    fn process_complete_line(&mut self, line: &str) {
        let trimmed = line.trim();
        let mut closed_fence = false;
        let mut closed_html = false;
        let fence_marker = fence_marker(line);
        let fence_line = fence_marker.is_some();
        if let Some(marker) = fence_marker {
            match self.fence_open {
                Some(open) if marker.closes(open) => {
                    self.fence_open = None;
                    closed_fence = true;
                }
                None => {
                    self.fence_open = Some(marker);
                    // Keep an enclosing list held. A zero-indent fence may
                    // interrupt it, but retaining the guard is conservative
                    // and also covers nested fenced blocks.
                }
                Some(_) => {}
            }
        }
        let fence_active = fence_line || self.fence_open.is_some();
        let html_active = !fence_active && self.process_html_block(line, &mut closed_html);
        let indented_code_active =
            !fence_active && !html_active && self.process_indented_code_line(line, trimmed);
        if fence_active || html_active || indented_code_active {
            if trimmed.is_empty() && self.fence_open.is_some() {
                // Blank lines inside a fence are code, not block separators.
            } else {
                self.prev_blank = false;
            }
        } else if trimmed.is_empty() {
            // Link labels cannot cross a blank line. An unmatched `[` before
            // this boundary is plain text, not document-global context.
            self.pending_reference_candidate.clear();
            self.prev_blank = true;
        } else {
            self.note_reference_context(line);
            if thematic_break_marker(trimmed) || atx_heading_marker(trimmed) {
                self.in_list_tail = false;
            } else if list_item_marker(line) && (self.in_list_tail || line_indent(line) <= 3) {
                if !self.in_list_tail {
                    self.in_list_tail = true;
                    self.list_guard = self.safe_end;
                }
            } else if self.in_list_tail && self.prev_blank && line_indent(line) < 2 {
                // An unindented paragraph after a blank line ends the list;
                // anything else (lazy continuation, indented item content)
                // keeps it open.
                self.in_list_tail = false;
            }
            self.prev_blank = false;
        }

        self.processed_len += line.len();
        if self.fence_open.is_none()
            && self.html_block_end.is_none()
            && !self.in_indented_code_tail
            && (trimmed.is_empty() || closed_fence || closed_html || atx_heading_marker(trimmed))
            && self.unresolved_references.is_empty()
            && self.pending_reference_candidate.is_empty()
        {
            self.safe_end = self.processed_len;
        }
    }

    fn process_html_block(&mut self, line: &str, closed: &mut bool) -> bool {
        if let Some(end) = self.html_block_end {
            if end.is_closed_by(line) {
                self.html_block_end = None;
                *closed = true;
            }
            return true;
        }
        let Some(end) = html_block_end(line) else {
            return false;
        };
        if end.is_closed_by(line) {
            *closed = true;
        } else {
            self.html_block_end = Some(end);
        }
        true
    }

    fn process_indented_code_line(&mut self, line: &str, trimmed: &str) -> bool {
        if self.in_indented_code_tail {
            if trimmed.is_empty() || line_indent(line) >= 4 {
                return true;
            }
            self.in_indented_code_tail = false;
        }
        if !trimmed.is_empty() && !self.in_list_tail && line_indent(line) >= 4 {
            self.in_indented_code_tail = true;
            return true;
        }
        false
    }

    fn note_reference_context(&mut self, line: &str) {
        if self.pending_reference_candidate.is_empty()
            && let Some(definition) = reference_definition_label(line)
        {
            self.has_reference_definitions = true;
            self.unresolved_references.remove(&definition);
            self.reference_definitions.insert(definition);
            return;
        }
        let mut scan = std::mem::take(&mut self.pending_reference_candidate);
        scan.push_str(line);
        let candidates = reference_candidate_labels(&scan);
        self.pending_reference_candidate = candidates.trailing_open;
        for candidate in candidates.labels {
            if !self.reference_definitions.contains(&candidate) {
                self.unresolved_references.insert(candidate);
            }
        }
    }

    fn stable_end(&self) -> usize {
        if !self.in_list_tail {
            return self.safe_end;
        }
        // The unterminated tail can already prove the list closed: after a
        // blank line, an unindented line whose first character can never form
        // a list-item marker is a paragraph that interrupts the list. The
        // ambiguous starters (`-`, `+`, `*`, digits) could still grow into a
        // sibling item, so they keep the hold.
        let partial_ends_list = self.prev_blank
            && line_indent(&self.pending_line) < 2
            && self
                .pending_line
                .trim_start_matches(' ')
                .chars()
                .next()
                .is_some_and(|ch| !matches!(ch, '-' | '+' | '*') && !ch.is_ascii_digit());
        if !partial_ends_list {
            return self.list_guard.min(self.safe_end);
        }
        self.safe_end
    }
}

/// Return the byte index of the longest conservative stable source prefix.
/// For growing streams, reuse [`StablePrefixTracker`] to scan each byte once.
pub fn stable_prefix_end(source: &str) -> usize {
    StablePrefixTracker::default().push(source)
}

fn line_indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// `---` / `***` / `___` style thematic break (3+ of one marker char, spaces
/// allowed between). Checked before the list-item marker so `- - -` is a
/// break, not a bullet.
fn thematic_break_marker(trimmed: &str) -> bool {
    let mut marker = None;
    let mut count = 0usize;
    for ch in trimmed.chars() {
        match (marker, ch) {
            (_, ' ' | '\t') => {}
            (None, '-' | '_' | '*') => {
                marker = Some(ch);
                count = 1;
            }
            (Some(open), _) if ch == open => count += 1,
            _ => return false,
        }
    }
    count >= 3
}

/// A line that starts a bullet (`-`/`+`/`*`) or ordered (`1.` / `1)`) list
/// item. Operates on the raw line; the caller decides how much indent is
/// allowed in context.
fn list_item_marker(line: &str) -> bool {
    let content = line.trim_end_matches(['\n', '\r']).trim_start_matches(' ');
    let mut chars = content.chars();
    match chars.next() {
        Some('-' | '+' | '*') => matches!(chars.next(), None | Some(' ' | '\t')),
        Some(ch) if ch.is_ascii_digit() => {
            let digits = content.chars().take_while(char::is_ascii_digit).count();
            if digits > 9 {
                return false;
            }
            let mut rest = content[digits..].chars();
            matches!(rest.next(), Some('.' | ')')) && matches!(rest.next(), None | Some(' ' | '\t'))
        }
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FenceMarker {
    ch: char,
    len: usize,
    can_close: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HtmlBlockEnd {
    Exact(&'static str),
    AsciiCaseInsensitive(&'static str),
}

impl HtmlBlockEnd {
    fn is_closed_by(self, line: &str) -> bool {
        match self {
            Self::Exact(needle) => line.contains(needle),
            Self::AsciiCaseInsensitive(needle) => line.to_ascii_lowercase().contains(needle),
        }
    }
}

fn html_block_end(line: &str) -> Option<HtmlBlockEnd> {
    let content = line.trim_start_matches(' ');
    if line.len() - content.len() > 3 || !content.starts_with('<') {
        return None;
    }
    let lower = content.to_ascii_lowercase();
    for (tag, end) in [
        ("<script", "</script>"),
        ("<pre", "</pre>"),
        ("<style", "</style>"),
        ("<textarea", "</textarea>"),
    ] {
        if lower.strip_prefix(tag).is_some_and(|rest| {
            rest.is_empty()
                || rest
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_whitespace() || ch == '>')
        }) {
            return Some(HtmlBlockEnd::AsciiCaseInsensitive(end));
        }
    }
    if content.starts_with("<!--") {
        Some(HtmlBlockEnd::Exact("-->"))
    } else if content.starts_with("<?") {
        Some(HtmlBlockEnd::Exact("?>"))
    } else if content.starts_with("<![CDATA[") {
        Some(HtmlBlockEnd::Exact("]]>"))
    } else if content
        .strip_prefix("<!")
        .and_then(|rest| rest.chars().next())
        .is_some_and(|ch| ch.is_ascii_uppercase())
    {
        Some(HtmlBlockEnd::Exact(">"))
    } else {
        None
    }
}

impl FenceMarker {
    fn closes(self, open: Self) -> bool {
        self.can_close && self.ch == open.ch && self.len >= open.len
    }
}

fn fence_marker(trimmed: &str) -> Option<FenceMarker> {
    let candidate = trimmed.strip_suffix('\n').unwrap_or(trimmed);
    let candidate = candidate.strip_suffix('\r').unwrap_or(candidate);
    let indent = candidate.len() - candidate.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }

    let candidate = &candidate[indent..];
    let mut chars = candidate.chars();
    let ch = chars.next()?;
    if ch != '`' && ch != '~' {
        return None;
    }
    let len = candidate
        .chars()
        .take_while(|candidate| *candidate == ch)
        .count();
    if len < 3 {
        return None;
    }
    let rest = &candidate[len..];
    let can_close = rest.chars().all(char::is_whitespace);

    // Opening backtick fences cannot contain backticks in the info string.
    if !can_close && ch == '`' && rest.contains('`') {
        return None;
    }

    Some(FenceMarker { ch, len, can_close })
}

fn atx_heading_marker(trimmed: &str) -> bool {
    let marker_len = trimmed.chars().take_while(|ch| *ch == '#').count();
    (1..=6).contains(&marker_len)
        && trimmed
            .chars()
            .nth(marker_len)
            .is_none_or(char::is_whitespace)
}

fn reference_definition_label(line: &str) -> Option<String> {
    let candidate = line.strip_prefix("   ").or_else(|| {
        line.strip_prefix("  ")
            .or_else(|| line.strip_prefix(' ').or(Some(line)))
    })?;
    let rest = candidate.strip_prefix('[')?;
    let close = rest.find("]:")?;
    normalize_reference_label(&rest[..close])
}

#[derive(Debug, Default)]
struct ReferenceCandidates {
    labels: Vec<String>,
    trailing_open: String,
}

fn reference_candidate_labels(line: &str) -> ReferenceCandidates {
    let mut candidates = ReferenceCandidates::default();
    let bytes = line.as_bytes();
    let mut idx = 0usize;
    while idx < bytes.len() {
        let Some(rel_open) = line[idx..].find('[') else {
            break;
        };
        let open = idx + rel_open;
        if is_task_marker_at(line, open) {
            idx = open + 3;
            continue;
        }

        let label_start = open + 1;
        let Some(rel_close) = line[label_start..].find(']') else {
            candidates.trailing_open.push_str(&line[open..]);
            break;
        };
        let close = label_start + rel_close;
        let Some(label) = normalize_reference_label(&line[label_start..close]) else {
            idx = close + 1;
            continue;
        };

        let after_close = close + 1;
        match line[after_close..].chars().next() {
            Some('(') => {
                idx = after_close + 1;
            }
            Some('[') => {
                let target_start = after_close + 1;
                let Some(rel_target_close) = line[target_start..].find(']') else {
                    candidates.trailing_open.push_str(&line[open..]);
                    break;
                };
                let target_close = target_start + rel_target_close;
                let target = if target_start == target_close {
                    label
                } else if let Some(target) =
                    normalize_reference_label(&line[target_start..target_close])
                {
                    target
                } else {
                    idx = target_close + 1;
                    continue;
                };
                candidates.labels.push(target);
                idx = target_close + 1;
            }
            _ => {
                candidates.labels.push(label);
                idx = after_close;
            }
        }
    }
    candidates
}

fn is_task_marker_at(line: &str, open: usize) -> bool {
    let before = line[..open].trim();
    let has_list_marker = before == "-"
        || before == "+"
        || before == "*"
        || before.strip_suffix('.').is_some_and(|digits| {
            !digits.is_empty() && digits.chars().all(|ch| ch.is_ascii_digit())
        });
    has_list_marker
        && matches!(
            line[open..].chars().take(3).collect::<Vec<_>>().as_slice(),
            ['[', ' ', ']'] | ['[', 'x' | 'X', ']']
        )
}

fn normalize_reference_label(label: &str) -> Option<String> {
    let normalized = label.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() || normalized.len() > 999 {
        None
    } else {
        Some(normalized.to_ascii_lowercase())
    }
}
