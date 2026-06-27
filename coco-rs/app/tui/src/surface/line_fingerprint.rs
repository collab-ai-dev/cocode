use std::hash::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;

use ratatui::text::Line;

/// Content hash of one rendered line (line style + alignment + span
/// content/styles).
///
/// Used for cheap equality between rows already inserted into native
/// scrollback and a later re-render of the same source — today only the
/// session header (the streamed-stable-prefix usage was removed by the
/// tui-v2 anchored finalize, §6.2). Process-local only — never persisted —
/// so `DefaultHasher`'s lack of cross-run stability is fine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RenderedLineFingerprint(u64);

pub(crate) fn fingerprint_lines(lines: &[Line<'_>]) -> Vec<RenderedLineFingerprint> {
    lines.iter().map(fingerprint_line).collect()
}

fn fingerprint_line(line: &Line<'_>) -> RenderedLineFingerprint {
    let mut hasher = DefaultHasher::new();
    line.style.hash(&mut hasher);
    line.alignment.hash(&mut hasher);
    line.spans.len().hash(&mut hasher);
    for span in &line.spans {
        span.content.as_ref().hash(&mut hasher);
        span.style.hash(&mut hasher);
    }
    RenderedLineFingerprint(hasher.finish())
}
