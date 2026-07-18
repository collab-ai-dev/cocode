//! Canonical display-width-aware wrapping for textarea source and projections.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::textarea_elements::ProjectedTextElement;

#[derive(Debug, Clone)]
pub(super) struct WrapAtom {
    pub(super) range: Range<usize>,
    pub(super) display_width: usize,
}

pub fn wrap_ranges(text: &str, width: u16) -> Vec<Range<usize>> {
    compute_wrapped_lines(text, width, &[])
}

pub fn wrap_ranges_with_elements(
    text: &str,
    width: u16,
    elements: &[ProjectedTextElement],
) -> Vec<Range<usize>> {
    let atoms = elements
        .iter()
        .filter_map(|element| {
            let display = text.get(element.range().clone())?;
            Some(WrapAtom {
                range: element.range().clone(),
                display_width: UnicodeWidthStr::width(display),
            })
        })
        .collect::<Vec<_>>();
    compute_wrapped_lines(text, width, &atoms)
}

pub(super) fn compute_wrapped_lines(
    text: &str,
    width: u16,
    atoms: &[WrapAtom],
) -> Vec<Range<usize>> {
    if text.is_empty() {
        return std::iter::once(0..0).collect();
    }
    let mut lines = Vec::new();
    let mut logical_start = 0usize;
    while logical_start <= text.len() {
        let logical_end = text[logical_start..]
            .find('\n')
            .map(|index| logical_start + index)
            .unwrap_or(text.len());
        wrap_logical_line(text, logical_start, logical_end, width, atoms, &mut lines);
        if logical_end == text.len() {
            break;
        }
        logical_start = logical_end + 1;
        if logical_start == text.len() {
            lines.push(text.len()..text.len());
            break;
        }
    }
    if lines.is_empty() {
        lines.push(0..0);
    }
    lines
}

fn wrap_logical_line(
    text: &str,
    start: usize,
    end: usize,
    width: u16,
    atoms: &[WrapAtom],
    out: &mut Vec<Range<usize>>,
) {
    if start == end || width == 0 {
        out.push(start..end);
        return;
    }
    let line_atoms = atoms
        .iter()
        .filter(|atom| atom.range.start >= start && atom.range.end <= end)
        .collect::<Vec<_>>();
    let units = display_units(text, start, end, &line_atoms);
    let limit = usize::from(width);
    if units.iter().map(|unit| unit.width).sum::<usize>() <= limit {
        out.push(start..end);
        return;
    }
    let mut col = 0usize;
    let mut chunk_start = 0usize;
    let mut break_at = None;
    let mut width_since_break = 0usize;
    for unit in units {
        let unit_width = unit.width.min(limit);
        let index = unit.range.start - start;
        if col + unit_width > limit && index > chunk_start {
            let cut = break_at
                .filter(|cut| *cut > chunk_start)
                .filter(|_| width_since_break + unit_width <= limit)
                .unwrap_or(index);
            out.push(start + chunk_start..start + cut);
            chunk_start = cut;
            col = if break_at == Some(cut) {
                width_since_break + unit_width
            } else {
                unit_width
            };
            break_at = None;
            width_since_break = 0;
        } else {
            col += unit_width;
            if break_at.is_some() {
                width_since_break += unit_width;
            }
        }
        if unit.whitespace {
            break_at = Some(unit.range.end - start);
            width_since_break = 0;
        }
    }
    if chunk_start < end - start {
        out.push(start + chunk_start..end);
    }
}

#[derive(Debug)]
struct DisplayUnit {
    range: Range<usize>,
    width: usize,
    whitespace: bool,
}

fn display_units(text: &str, start: usize, end: usize, atoms: &[&WrapAtom]) -> Vec<DisplayUnit> {
    let mut units = Vec::new();
    let mut position = start;
    for atom in atoms {
        if position < atom.range.start {
            push_plain_units(text, position, atom.range.start, &mut units);
        }
        units.push(DisplayUnit {
            range: atom.range.clone(),
            width: atom.display_width,
            whitespace: false,
        });
        position = atom.range.end;
    }
    if position < end {
        push_plain_units(text, position, end, &mut units);
    }
    units
}

fn push_plain_units(text: &str, start: usize, end: usize, out: &mut Vec<DisplayUnit>) {
    let Some(plain) = text.get(start..end) else {
        return;
    };
    for (offset, grapheme) in plain.grapheme_indices(true) {
        let unit_start = start + offset;
        out.push(DisplayUnit {
            range: unit_start..unit_start + grapheme.len(),
            width: UnicodeWidthStr::width(grapheme),
            whitespace: grapheme.chars().all(char::is_whitespace),
        });
    }
}
