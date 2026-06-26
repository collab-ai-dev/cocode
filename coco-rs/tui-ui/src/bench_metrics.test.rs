use super::*;

#[test]
fn clean_cache_hit_requires_hit_and_zero_rerenders() {
    assert!(is_clean_cache_hit(true, 0));
    assert!(
        !is_clean_cache_hit(true, 1),
        "a cache hit that re-renders is not clean"
    );
    assert!(!is_clean_cache_hit(false, 0), "a miss is never a clean hit");
}

#[test]
fn changed_cell_ratio_handles_empty_partial_and_full() {
    assert_eq!(
        changed_cell_ratio(0, 0),
        0.0,
        "empty surface wastes nothing"
    );
    assert_eq!(changed_cell_ratio(0, 100), 0.0);
    assert_eq!(changed_cell_ratio(25, 100), 0.25);
    assert_eq!(changed_cell_ratio(100, 100), 1.0);
}

#[test]
fn minimal_repaint_is_zero_changed_cells() {
    assert!(is_minimal_repaint(0));
    assert!(!is_minimal_repaint(1));
}
