use super::*;
use pretty_assertions::assert_eq;

#[test]
fn pager_window_clamps_negative_scroll_to_start() {
    let window = pager_window(10, -3, 4);

    assert_eq!(
        window,
        PagerWindow {
            offset: 0,
            end: 4,
            total: 10,
        }
    );
    assert_eq!(window.range(), 0..4);
    assert_eq!(window.position_suffix(), " [1/10]");
}

#[test]
fn pager_window_clamps_past_end_to_empty_tail() {
    let window = pager_window(10, 99, 4);

    assert_eq!(
        window,
        PagerWindow {
            offset: 10,
            end: 10,
            total: 10,
        }
    );
    assert_eq!(window.range(), 10..10);
    assert_eq!(window.position_suffix(), " [11/10]");
}

#[test]
fn pager_window_empty_has_no_position_suffix() {
    let window = pager_window(0, 5, 4);

    assert_eq!(window.range(), 0..0);
    assert_eq!(window.position_suffix(), "");
}
