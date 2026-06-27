//! Shared pager view model helpers.

use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PagerWindow {
    pub(crate) offset: usize,
    pub(crate) end: usize,
    pub(crate) total: usize,
}

impl PagerWindow {
    pub(crate) fn range(self) -> Range<usize> {
        self.offset..self.end
    }

    pub(crate) fn position_suffix(self) -> String {
        if self.total == 0 {
            String::new()
        } else {
            format!(" [{}/{}]", self.offset + 1, self.total)
        }
    }
}

pub(crate) fn pager_window(total: usize, scroll: i32, height: usize) -> PagerWindow {
    let offset = (scroll.max(0) as usize).min(total);
    let end = offset.saturating_add(height).min(total);
    PagerWindow { offset, end, total }
}

#[cfg(test)]
#[path = "pager.test.rs"]
mod tests;
