use super::*;

#[test]
fn centered_modal_area_handles_zero_and_narrow_areas() {
    let bounds = ModalBounds::new(70, 80, 60, 112, 18, 32);
    assert_eq!(
        centered_modal_area(Rect::new(0, 0, 0, 0), bounds),
        Rect::new(0, 0, 0, 0)
    );

    let area = centered_modal_area(Rect::new(0, 0, 1, 1), bounds);
    assert_eq!(area.width, 1);
    assert_eq!(area.height, 1);

    let area = centered_modal_area(Rect::new(0, 0, 40, 10), bounds);
    assert!(area.width <= 38);
    assert!(area.height <= 8);
}

#[test]
fn centered_fixed_area_clamps_requested_size_to_available_area() {
    let fixed = centered_fixed_area(Rect::new(0, 0, 12, 5), 40, 20);

    assert_eq!(fixed, Rect::new(1, 1, 10, 3));
}

#[test]
fn visible_window_keeps_selected_row_in_bounds() {
    assert_eq!(visible_window(0, 0, 5), 0..0);
    assert_eq!(visible_window(0, 10, 0), 0..0);
    assert_eq!(visible_window(0, 3, 10), 0..3);
    assert_eq!(visible_window(5, 10, 4), 3..7);
    assert_eq!(visible_window(99, 10, 4), 6..10);
}

#[test]
fn selected_in_bounds_clamps_negative_and_large_indices() {
    assert_eq!(selected_in_bounds(0, 0), None);
    assert_eq!(selected_in_bounds(-10, 3), Some(0));
    assert_eq!(selected_in_bounds(1, 3), Some(1));
    assert_eq!(selected_in_bounds(99, 3), Some(2));
}

#[test]
fn truncate_to_width_is_unicode_width_aware() {
    assert_eq!(truncate_to_width("abcdef", 0), "");
    assert_eq!(truncate_to_width("abcdef", 1), "…");
    assert_eq!(truncate_to_width("abcdef", 4), "abc…");
    assert_eq!(truncate_to_width("a界b", 3), "a…");
    assert_eq!(text_width(&truncate_to_width("a界b", 3)), 2);
}
