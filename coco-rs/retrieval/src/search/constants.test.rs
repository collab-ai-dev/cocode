use super::*;

#[test]
fn test_seconds_per_day() {
    assert_eq!(SECONDS_PER_DAY, 60.0 * 60.0 * 24.0);
}

#[test]
fn test_ln_2_precision() {
    // Verify LN_2 is close to actual ln(2)
    let actual_ln2 = 2.0_f32.ln();
    assert!((LN_2 - actual_ln2).abs() < 1e-6);
}

#[test]
fn test_rrf_k_positive() {
    const { assert!(DEFAULT_RRF_K > 0.0) };
}

#[test]
fn test_recency_half_life_positive() {
    const { assert!(DEFAULT_RECENCY_HALF_LIFE_DAYS > 0.0) };
}
