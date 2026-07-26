use pretty_assertions::assert_eq;

use super::MIN_TIP_WIDTH;
use super::TIPS;
use super::Tip;
use super::tip_for_day;
use crate::i18n::locale_test_guard;
use crate::keymap::KEYMAP;

#[test]
fn every_tip_points_at_a_real_keymap_entry() {
    // The guard that keeps the catalog honest: a tip advertising a key that no
    // longer exists is worse than no tip, because the user tries it.
    for tip in TIPS {
        let id = tip.keymap_id();
        assert!(
            KEYMAP.iter().any(|entry| entry.id == id),
            "{tip:?} references missing keymap entry {id}",
        );
    }
}

#[test]
fn every_tip_resolves_in_both_locales_and_names_its_key() {
    for locale in ["en", "zh-CN"] {
        let _locale = locale_test_guard(locale);
        for tip in TIPS {
            let text = tip.text();
            assert!(
                !text.starts_with("tip."),
                "{tip:?} has no {locale} translation (got {text})",
            );
            // The combo is interpolated, not baked in — an un-substituted
            // placeholder means the string and the code disagree.
            assert!(
                !text.contains("%{combo}"),
                "{tip:?} left its combo placeholder unsubstituted in {locale}",
            );
        }
    }
}

#[test]
fn the_catalog_has_no_duplicates() {
    for (index, tip) in TIPS.iter().enumerate() {
        assert!(
            !TIPS[index + 1..].contains(tip),
            "{tip:?} appears twice in the rotation",
        );
    }
}

#[test]
fn rotation_advances_once_per_day_and_wraps() {
    let first = tip_for_day(/*enabled*/ true, /*width*/ 120, 0);
    let second = tip_for_day(/*enabled*/ true, /*width*/ 120, 1);
    assert_ne!(first, second);
    // A full cycle returns to the start.
    assert_eq!(
        tip_for_day(/*enabled*/ true, /*width*/ 120, TIPS.len() as i64),
        first,
    );
}

#[test]
fn the_same_day_always_yields_the_same_tip() {
    // Restarting coco must not reshuffle the line; that is what makes it read
    // as information rather than noise.
    let day = 19_000;
    let tip = tip_for_day(/*enabled*/ true, /*width*/ 120, day);
    for _ in 0..5 {
        assert_eq!(tip_for_day(/*enabled*/ true, /*width*/ 120, day), tip);
    }
}

#[test]
fn a_pre_epoch_clock_still_lands_inside_the_catalog() {
    // A machine with a badly wrong date must not index out of bounds.
    assert!(tip_for_day(/*enabled*/ true, /*width*/ 120, -3).is_some());
    assert!(tip_for_day(/*enabled*/ true, /*width*/ 120, i64::MIN + 1).is_some());
}

#[test]
fn tips_are_suppressed_when_disabled_or_on_a_narrow_terminal() {
    assert_eq!(tip_for_day(/*enabled*/ false, /*width*/ 120, 0), None);
    assert_eq!(tip_for_day(/*enabled*/ true, MIN_TIP_WIDTH - 1, 0), None,);
    assert!(tip_for_day(/*enabled*/ true, MIN_TIP_WIDTH, 0).is_some());
}

#[test]
fn the_newline_tip_names_a_combo_the_terminal_can_report() {
    let _locale = locale_test_guard("en");
    // Resolved through the keymap rather than hardcoded, so the P0 fix for
    // legacy terminals reaches the tip too.
    let text = Tip::Newline.text();
    assert!(text.contains("Enter"), "{text}");
}
