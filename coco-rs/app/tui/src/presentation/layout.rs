//! Layout helpers shared by presentation surfaces.

use std::ops::Range;

use ratatui::layout::Constraint;
use ratatui::layout::Rect;

// Width-aware text helpers: the canonical, tested implementation lives in
// `coco_tui_ui::truncate`. Re-exported so existing `layout::{text_width,
// truncate_to_width}` call sites keep working without a second copy here.
pub(crate) use coco_tui_ui::truncate::display_width as text_width;
pub(crate) use coco_tui_ui::truncate::truncate_to_width;

/// Bounds for a centered state.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ModalBounds {
    pub(crate) width_percent: u16,
    pub(crate) height_percent: u16,
    pub(crate) min_width: u16,
    pub(crate) max_width: u16,
    pub(crate) min_height: u16,
    pub(crate) max_height: u16,
}

impl ModalBounds {
    pub(crate) const fn new(
        width_percent: u16,
        height_percent: u16,
        min_width: u16,
        max_width: u16,
        min_height: u16,
        max_height: u16,
    ) -> Self {
        Self {
            width_percent,
            height_percent,
            min_width,
            max_width,
            min_height,
            max_height,
        }
    }
}

/// Center an state inside `area`, clamping to available space first.
pub(crate) fn centered_modal_area(area: Rect, bounds: ModalBounds) -> Rect {
    if area.width == 0 || area.height == 0 {
        return area;
    }

    let max_width = area.width.saturating_sub(2).max(1);
    let max_height = area.height.saturating_sub(2).max(1);
    let preferred_width = area.width.saturating_mul(bounds.width_percent) / 100;
    let preferred_height = area.height.saturating_mul(bounds.height_percent) / 100;
    let width = clamp_modal_len(
        preferred_width,
        bounds.min_width,
        bounds.max_width,
        max_width,
    );
    let height = clamp_modal_len(
        preferred_height,
        bounds.min_height,
        bounds.max_height,
        max_height,
    );

    area.centered(Constraint::Length(width), Constraint::Length(height))
}

/// Center a fixed-size state, clamping it inside `area`.
pub(crate) fn centered_fixed_area(area: Rect, width: u16, height: u16) -> Rect {
    centered_modal_area(
        area,
        ModalBounds::new(100, 100, width, width, height, height),
    )
}

#[cfg(test)]
pub(crate) fn inner_size(area: Rect) -> (usize, usize) {
    (
        area.width.saturating_sub(2) as usize,
        area.height.saturating_sub(2) as usize,
    )
}

pub(crate) fn selected_in_bounds(selected: i32, row_count: usize) -> Option<usize> {
    if row_count == 0 {
        return None;
    }
    Some((selected.max(0) as usize).min(row_count - 1))
}

/// Compute the visible row range while keeping the selected row in bounds.
pub(crate) fn visible_window(selected: usize, row_count: usize, height: usize) -> Range<usize> {
    if row_count == 0 || height == 0 {
        return 0..0;
    }

    let selected = selected.min(row_count - 1);
    let visible_len = height.min(row_count);
    let start = if row_count <= visible_len {
        0
    } else {
        selected
            .saturating_sub(visible_len / 2)
            .min(row_count - visible_len)
    };
    start..start + visible_len
}

fn clamp_modal_len(preferred: u16, min: u16, max: u16, available: u16) -> u16 {
    let upper = max.min(available);
    let lower = min.min(upper);
    preferred.clamp(lower, upper)
}

#[cfg(test)]
#[path = "layout.test.rs"]
mod tests;
