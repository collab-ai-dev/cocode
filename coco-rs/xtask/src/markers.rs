//! Splice generated content between `<!-- BEGIN GENERATED: <id> -->` and
//! `<!-- END GENERATED: <id> -->`, leaving every other byte of the file alone.
//!
//! A missing or duplicated marker is an error: silently generating nothing is
//! how a drift gate rots into a no-op.

use anyhow::Result;
use anyhow::bail;

pub fn begin_marker(id: &str) -> String {
    format!("<!-- BEGIN GENERATED: {id} -->")
}

pub fn end_marker(id: &str) -> String {
    format!("<!-- END GENERATED: {id} -->")
}

/// Replace the body between the `id` markers with `body`, surrounded by the
/// blank lines the docs already use. `label` names the file in errors.
pub fn splice(source: &str, id: &str, body: &str, label: &str) -> Result<String> {
    let begin = begin_marker(id);
    let end = end_marker(id);

    let begin_count = source.matches(begin.as_str()).count();
    let end_count = source.matches(end.as_str()).count();
    if begin_count != 1 || end_count != 1 {
        bail!(
            "{label}: expected exactly one `{begin}` and one `{end}`, \
             found {begin_count} begin / {end_count} end marker(s)"
        );
    }

    // `find` on an ASCII marker yields a char boundary, and `begin_at +
    // begin.len()` lands just past it — both are safe slice indices.
    let Some(begin_at) = source.find(begin.as_str()) else {
        bail!("{label}: missing `{begin}`");
    };
    let Some(end_at) = source.find(end.as_str()) else {
        bail!("{label}: missing `{end}`");
    };

    let body_start = begin_at + begin.len();
    if end_at < body_start {
        bail!("{label}: `{end}` appears before `{begin}`");
    }

    let head = &source[..body_start];
    let tail = &source[end_at..];
    Ok(format!("{head}\n\n{body}\n\n{tail}"))
}

#[cfg(test)]
#[path = "markers.test.rs"]
mod tests;
