//! Cancellation-aware persisted-transcript text search.

use std::io::BufRead;
use std::ops::Range;

use super::Entry;
use super::MAX_TRANSCRIPT_READ_BYTES;
use super::TranscriptStore;
use super::messages_from_transcript_entry;
use super::parse_entry;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptTextMatch {
    pub line: String,
    pub match_range: Range<usize>,
}

/// Return the matching byte range in the original UTF-8 text. Lowercasing a
/// Unicode scalar may change its byte length (or expand to multiple scalars),
/// so offsets from a separately-lowercased `String` cannot be used directly.
pub(crate) fn case_insensitive_match_range(
    text: &str,
    needle_lowercase: &str,
) -> Option<Range<usize>> {
    if needle_lowercase.is_empty() {
        return Some(0..0);
    }
    let mut lowercase = String::new();
    let mut original_ranges = Vec::new();
    for (start, ch) in text.char_indices() {
        let end = start + ch.len_utf8();
        let lowered = ch.to_lowercase().collect::<String>();
        lowercase.push_str(&lowered);
        original_ranges.extend(std::iter::repeat_n(start..end, lowered.len()));
    }
    let lowercase_start = lowercase.find(needle_lowercase)?;
    let lowercase_end = lowercase_start + needle_lowercase.len();
    let original_start = original_ranges.get(lowercase_start)?.start;
    let original_end = original_ranges.get(lowercase_end.checked_sub(1)?)?.end;
    Some(original_start..original_end)
}

pub(super) fn find_transcript_text(
    store: &TranscriptStore,
    session_id: &str,
    needle_lowercase: &str,
    is_cancelled: &mut dyn FnMut() -> bool,
) -> crate::Result<Option<TranscriptTextMatch>> {
    let path = store.transcript_path(session_id);
    if !path.exists() {
        return Err(crate::SessionError::TranscriptNotFound { path });
    }
    let meta = std::fs::metadata(&path)?;
    if meta.len() > MAX_TRANSCRIPT_READ_BYTES {
        return Err(crate::SessionError::generic(format!(
            "transcript file too large ({} bytes, max {MAX_TRANSCRIPT_READ_BYTES}): {}",
            meta.len(),
            path.display(),
        )));
    }

    let reader = std::io::BufReader::new(std::fs::File::open(path)?);
    for line in reader.lines() {
        if is_cancelled() {
            return Ok(None);
        }
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Entry::Transcript(entry) = parse_entry(&line) else {
            continue;
        };
        for message in messages_from_transcript_entry(&entry) {
            if is_cancelled() {
                return Ok(None);
            }
            let text = coco_messages::wrapping::extract_text_from_message(&message);
            if let Some((line, match_range)) = text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .find_map(|line| {
                    case_insensitive_match_range(line, needle_lowercase)
                        .map(|match_range| (line, match_range))
                })
            {
                return Ok(Some(TranscriptTextMatch {
                    line: line.to_string(),
                    match_range,
                }));
            }
        }
    }
    Ok(None)
}
