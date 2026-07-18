use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;

use coco_tui_ui::display::SyntaxHighlighting;
use coco_tui_ui::style::UiStyles;
use coco_tui_ui::theme::Theme;
use coco_tui_ui::theme::ThemeName;

use super::ClosedFenceMemo;
use super::HighlightMode;
use super::Highlighted;
use super::highlight_code;
use super::prewarm_highlighting;

/// The streaming fence slot and its memo are process-global, so tests that
/// assert on *which* fence owns the slot cannot interleave with each other.
/// Every test that renders in [`HighlightMode::Streaming`] takes this first.
/// Poisoning is ignored: a panicking sibling says nothing about slot validity,
/// and the slot is reset below anyway.
fn streaming_test_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Force the shared slot to a known-empty state so a test's first streaming
/// call is deterministically a rebuild (and therefore a memo insert), whatever
/// a previously-run test left behind.
fn reset_streaming_slot() {
    *super::streaming_fence_slot().lock().expect("slot lock") = None;
}

/// Which fence currently owns the slot, by language tag.
fn slot_owner() -> Option<String> {
    super::streaming_fence_slot()
        .lock()
        .expect("slot lock")
        .as_ref()
        .map(|slot| slot.lang.clone())
}

fn stream(code: &str, lang: &str, styles: UiStyles<'_>) -> Option<Highlighted> {
    highlight_code(
        code,
        lang,
        styles,
        SyntaxHighlighting::Full,
        HighlightMode::Streaming,
    )
}

#[test]
fn highlights_known_language() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let out = highlight_code(
        "fn main() {}\n",
        "rust",
        styles,
        SyntaxHighlighting::Full,
        HighlightMode::Committed,
    );
    let lines = out.expect("rust is a known grammar");
    assert!(!lines.is_empty());
    // The first line carries the `fn` keyword as a styled span.
    let text: String = lines[0].iter().map(|s| s.content.as_ref()).collect();
    assert!(text.contains("fn"), "expected keyword in {text:?}");
}

#[test]
fn lite_tier_highlights_only_prewarmed_grammars() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    // In `LITE_GRAMMARS`: shell resolves to the bash grammar → highlighted.
    assert!(
        highlight_code(
            "echo hi\n",
            "bash",
            styles,
            SyntaxHighlighting::Lite,
            HighlightMode::Committed,
        )
        .is_some(),
        "lite must still highlight the prewarmed hot-path grammars"
    );
    // Not in `LITE_GRAMMARS`: rust falls back to plain (None) so its grammar
    // is never compiled — the whole point of the memory cap.
    assert!(
        highlight_code(
            "fn main() {}\n",
            "rust",
            styles,
            SyntaxHighlighting::Lite,
            HighlightMode::Committed,
        )
        .is_none(),
        "lite must not highlight languages outside the prewarm set"
    );
    // Off blocks everything, including the prewarmed grammars.
    assert!(
        highlight_code(
            "echo hi\n",
            "bash",
            styles,
            SyntaxHighlighting::Off,
            HighlightMode::Committed,
        )
        .is_none(),
        "off disables highlighting entirely"
    );
}

#[test]
fn keyword_token_uses_code_keyword_color_without_bold() {
    // Regression guard on the token→style mapping itself (independent of which
    // syntect scope a given word lands in): keywords paint with `code_keyword`
    // and stay unbolded. The old ANSI-Magenta + BOLD combo read as a harsh red;
    // both of claude-code's highlighters leave keywords unbolded.
    use ratatui::style::Modifier;

    use super::CodeToken;

    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let style = CodeToken::Keyword.style(styles);
    assert_eq!(style.fg, Some(theme.code_keyword));
    assert!(
        !style.add_modifier.contains(Modifier::BOLD),
        "keyword must not be bold, got {:?}",
        style.add_modifier
    );
}

#[test]
fn two_face_extended_grammars_resolve() {
    // Pin the two-face adoption: these languages are absent from syntect's
    // stock default set and were dead lookups before (the alias table mapped
    // "ts"→"TypeScript" against a bundle that had no TypeScript grammar).
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    for (code, lang) in [
        ("const x: number = 1;\n", "ts"),
        ("const el = <div/>;\n", "tsx"),
        ("[package]\nname = \"x\"\n", "toml"),
        ("FROM rust:1 AS build\n", "dockerfile"),
    ] {
        assert!(
            highlight_code(
                code,
                lang,
                styles,
                SyntaxHighlighting::Full,
                HighlightMode::Committed
            )
            .is_some(),
            "expected a grammar for {lang:?}"
        );
    }
}

#[test]
fn test_streaming_checkpoint_matches_fresh_tokenize() {
    // Multi-line string state is the trap: a checkpoint that drops parser
    // state between lines would color the continuation as code, not string.
    // Feed the block in growing prefixes — including partial lines — and pin
    // every streaming snapshot to a fresh committed tokenize of that prefix.
    let _serial = streaming_test_lock();
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let code = "let s = \"first\nsecond line\nthird\";\nfn after() {}\nlet t = 1;\n";
    for end in 1..=code.len() {
        if !code.is_char_boundary(end) {
            continue;
        }
        let prefix = &code[..end];
        let streamed = highlight_code(prefix, "rust", styles, SyntaxHighlighting::Full, {
            HighlightMode::Streaming
        })
        .expect("rust grammar");
        let fresh = super::highlight_uncached(prefix, "rust", styles).expect("rust grammar");
        assert_eq!(
            streamed.as_ref(),
            &fresh,
            "streaming checkpoint diverged from fresh tokenize at byte {end}"
        );
    }
}

#[test]
fn test_streaming_mode_does_not_pollute_committed_lru() {
    let _serial = streaming_test_lock();
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    // Prime the committed LRU with a block, then run streaming snapshots of a
    // DIFFERENT block; the committed entry must still be served Arc-identical
    // (i.e. not evicted/cleared by streaming traffic).
    let committed = highlight_code(
        "fn keep_me() {}\n",
        "rust",
        styles,
        SyntaxHighlighting::Full,
        HighlightMode::Committed,
    )
    .expect("rust grammar");
    for end in ["let x", "let x = 1;\n", "let x = 1;\nlet y = 2;\n"] {
        let _ = highlight_code(end, "rust", styles, SyntaxHighlighting::Full, {
            HighlightMode::Streaming
        });
    }
    let again = highlight_code(
        "fn keep_me() {}\n",
        "rust",
        styles,
        SyntaxHighlighting::Full,
        HighlightMode::Committed,
    )
    .expect("rust grammar");
    assert!(
        std::sync::Arc::ptr_eq(&committed, &again),
        "committed LRU entry must survive streaming renders"
    );
}

#[test]
fn test_streaming_multi_fence_tail_serves_closed_fence_from_memo() {
    // The multi-fence tail — a CLOSED fence rendered every frame beside a still
    // growing OPEN one — is the shape the single slot cannot hold: the two calls
    // alternate and each steals the slot, so before the memo BOTH re-tokenized
    // in full every frame. The closed fence's content never changes, so from its
    // second frame on it must come back memoized: an `Arc::ptr_eq` result is
    // proof no re-tokenize happened.
    let _serial = streaming_test_lock();
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let closed = "fn multi_fence_closed_probe() { let a = 1; }\n";
    reset_streaming_slot();

    // Frame 1: closed fence tokenizes and memoizes, then the open fence takes
    // the slot away from it.
    let first = stream(closed, "rust", styles).expect("rust grammar");
    let _ = stream("open_a = 1\n", "python", styles).expect("python grammar");
    // Frame 2: same closed content → memo.
    let second = stream(closed, "rust", styles).expect("rust grammar");

    assert!(
        Arc::ptr_eq(&first, &second),
        "a closed fence sharing the tail must be served from the memo, not re-tokenized"
    );
    // Memoized spans must still be the spans a fresh tokenize would produce.
    let fresh = super::highlight_uncached(closed, "rust", styles).expect("rust grammar");
    assert_eq!(second.as_ref(), &fresh);
}

#[test]
fn test_streaming_memo_hit_leaves_slot_with_open_fence() {
    // The load-bearing property: a memo hit must not touch the slot. That is
    // what lets the open fence keep its checkpoint (and extend O(delta)) while a
    // closed sibling renders between its frames.
    let _serial = streaming_test_lock();
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let closed = "fn slot_ownership_probe() { let b = 2; }\n";
    reset_streaming_slot();

    // Prime the memo with the closed fence, then hand the slot to the open one.
    let _ = stream(closed, "rust", styles).expect("rust grammar");
    let _ = stream("open_b = 1\n", "python", styles).expect("python grammar");
    assert_eq!(
        slot_owner().as_deref(),
        Some("python"),
        "the most recent rebuild should own the slot"
    );

    // This call is served from the memo and must leave the slot alone.
    let _ = stream(closed, "rust", styles).expect("rust grammar");
    assert_eq!(
        slot_owner().as_deref(),
        Some("python"),
        "a memo hit must not steal the slot from the open fence"
    );

    // And the open fence therefore still extends its checkpoint rather than
    // rebuilding: its next snapshot is a prefix-extension of the slot content.
    let grown = stream("open_b = 1\nopen_b += 2\n", "python", styles).expect("python grammar");
    assert_eq!(
        slot_owner().as_deref(),
        Some("python"),
        "the open fence must still hold the slot after growing"
    );
    let fresh = super::highlight_uncached("open_b = 1\nopen_b += 2\n", "python", styles)
        .expect("python grammar");
    assert_eq!(
        grown.as_ref(),
        &fresh,
        "the extended checkpoint must match a fresh tokenize"
    );
}

#[test]
fn test_streaming_memo_key_includes_theme() {
    // Same fence, different palette: serving theme A's spans under theme B would
    // paint stale colors after a live theme switch.
    let _serial = streaming_test_lock();
    let code = "fn streaming_memo_theme_probe() {}\n";
    let dark = Theme::from_name(ThemeName::Dark);
    let light = Theme::from_name(ThemeName::Light);
    reset_streaming_slot();

    let a = stream(code, "rust", UiStyles::new(&dark)).expect("dark");
    // Same content and language, so this rebuilds via `theme_changed` — the memo
    // must miss on the new theme's key rather than serve `a`.
    let b = stream(code, "rust", UiStyles::new(&light)).expect("light");
    assert!(
        !Arc::ptr_eq(&a, &b),
        "a theme change must not serve another theme's memoized spans"
    );
}

#[test]
fn test_closed_fence_memo_evicts_least_recently_used_over_byte_budget() {
    let mut memo = ClosedFenceMemo::default();
    let empty = || -> Highlighted { Arc::new(Vec::new()) };
    // Three entries at just over a third of the budget: the third insert must
    // push the total past the cap and force exactly one eviction.
    let third = super::CLOSED_MEMO_CAP_BYTES / 3 + 1;
    memo.put(1, empty(), third);
    memo.put(2, empty(), third);
    // Touch 1 so 2 is now the least-recently-used entry.
    assert!(memo.get(1).is_some());
    memo.put(3, empty(), third);

    assert!(
        memo.get(2).is_none(),
        "the least-recently-used entry must be evicted once the byte budget is exceeded"
    );
    assert!(
        memo.get(1).is_some(),
        "a touched entry must outlive an untouched one"
    );
    assert!(memo.get(3).is_some(), "the new entry must be retained");
    assert!(
        memo.bytes <= super::CLOSED_MEMO_CAP_BYTES,
        "accounting must stay within the budget, got {}",
        memo.bytes
    );
}

#[test]
fn unknown_language_falls_back_to_none() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    assert!(
        highlight_code(
            "some text\n",
            "definitely-not-a-language",
            styles,
            SyntaxHighlighting::Full,
            HighlightMode::Committed
        )
        .is_none()
    );
}

#[test]
fn disabled_highlighting_returns_none() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    assert!(
        highlight_code(
            "fn main() {}\n",
            "rust",
            styles,
            SyntaxHighlighting::Off,
            HighlightMode::Committed
        )
        .is_none()
    );
}

#[test]
fn cache_hit_returns_ptr_equal_arc() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    // Unique content so no sibling test populated this key first.
    let code = "fn cache_hit_returns_ptr_equal_arc() {}\n";
    let a = highlight_code(
        code,
        "rust",
        styles,
        SyntaxHighlighting::Full,
        HighlightMode::Committed,
    )
    .expect("a");
    let b = highlight_code(
        code,
        "rust",
        styles,
        SyntaxHighlighting::Full,
        HighlightMode::Committed,
    )
    .expect("b");
    assert!(
        Arc::ptr_eq(&a, &b),
        "second call must reuse the cached Arc (refcount bump, no re-tokenize)"
    );
}

#[test]
fn cache_key_includes_theme() {
    // Same code + language, different theme. If the key ignored the theme, the
    // second call would HIT the first theme's entry and return a ptr-equal Arc;
    // asserting non-equality proves the theme is part of the key.
    let code = "fn cache_key_includes_theme() {}\n";
    let t1 = Theme::from_name(ThemeName::Dark);
    let t2 = Theme::from_name(ThemeName::Light);
    let a = highlight_code(
        code,
        "rust",
        UiStyles::new(&t1),
        SyntaxHighlighting::Full,
        HighlightMode::Committed,
    )
    .expect("a");
    let b = highlight_code(
        code,
        "rust",
        UiStyles::new(&t2),
        SyntaxHighlighting::Full,
        HighlightMode::Committed,
    )
    .expect("b");
    assert!(
        !Arc::ptr_eq(&a, &b),
        "a different theme must not reuse another theme's cached highlight"
    );
}

#[test]
fn cache_key_includes_code() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let a = highlight_code(
        "fn aaa_distinct() {}\n",
        "rust",
        styles,
        SyntaxHighlighting::Full,
        HighlightMode::Committed,
    )
    .expect("a");
    let b = highlight_code(
        "fn bbb_distinct() {}\n",
        "rust",
        styles,
        SyntaxHighlighting::Full,
        HighlightMode::Committed,
    )
    .expect("b");
    assert!(
        !Arc::ptr_eq(&a, &b),
        "distinct code must be a distinct cache entry"
    );
}

#[test]
fn prewarm_highlighting_compiles_grammars_without_panicking() {
    prewarm_highlighting(SyntaxHighlighting::Lite);
    // Warmed grammars still highlight correctly afterwards.
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let highlighted = highlight_code(
        "# title\n",
        "md",
        styles,
        SyntaxHighlighting::Full,
        HighlightMode::Committed,
    )
    .expect("markdown highlights after prewarm");
    assert!(!highlighted.is_empty());
}
