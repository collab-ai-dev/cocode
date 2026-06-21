use std::collections::VecDeque;

use super::MAX_ROWS;
use super::QueueStatusWidget;
use crate::state::session::QueuedCommandDisplay;

fn queued(n: usize) -> VecDeque<QueuedCommandDisplay> {
    (0..n)
        .map(|i| QueuedCommandDisplay {
            id: format!("id-{i}"),
            preview: format!("msg {i}"),
            editable: true,
        })
        .collect()
}

#[test]
fn height_is_zero_when_empty() {
    assert_eq!(QueueStatusWidget::height(&queued(0)), 0);
}

#[test]
fn height_is_one_row_per_command_plus_top_margin() {
    // +1 for the blank top-margin row (TS marginTop={1}).
    assert_eq!(QueueStatusWidget::height(&queued(1)), 2);
    assert_eq!(QueueStatusWidget::height(&queued(3)), 4);
}

#[test]
fn height_caps_at_max_rows_plus_margin() {
    assert_eq!(
        QueueStatusWidget::height(&queued(MAX_ROWS + 5)),
        (MAX_ROWS + 1) as u16
    );
}
