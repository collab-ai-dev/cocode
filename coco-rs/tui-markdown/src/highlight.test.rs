use std::sync::Arc;

use coco_tui_ui::display::SyntaxHighlighting;
use coco_tui_ui::style::UiStyles;
use coco_tui_ui::theme::Theme;
use coco_tui_ui::theme::ThemeName;

use super::HighlightMode;
use super::highlight_code;
use super::prewarm_highlighting;

#[test]
fn highlights_known_language() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let out = highlight_code(
        "fn main() {}\n",
        "rust",
        styles,
        SyntaxHighlighting::Enabled,
        HighlightMode::Committed,
    );
    let lines = out.expect("rust is a known grammar");
    assert!(!lines.is_empty());
    // The first line carries the `fn` keyword as a styled span.
    let text: String = lines[0].iter().map(|s| s.content.as_ref()).collect();
    assert!(text.contains("fn"), "expected keyword in {text:?}");
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
                SyntaxHighlighting::Enabled,
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
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let code = "let s = \"first\nsecond line\nthird\";\nfn after() {}\nlet t = 1;\n";
    for end in 1..=code.len() {
        if !code.is_char_boundary(end) {
            continue;
        }
        let prefix = &code[..end];
        let streamed = highlight_code(prefix, "rust", styles, SyntaxHighlighting::Enabled, {
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
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    // Prime the committed LRU with a block, then run streaming snapshots of a
    // DIFFERENT block; the committed entry must still be served Arc-identical
    // (i.e. not evicted/cleared by streaming traffic).
    let committed = highlight_code(
        "fn keep_me() {}\n",
        "rust",
        styles,
        SyntaxHighlighting::Enabled,
        HighlightMode::Committed,
    )
    .expect("rust grammar");
    for end in ["let x", "let x = 1;\n", "let x = 1;\nlet y = 2;\n"] {
        let _ = highlight_code(end, "rust", styles, SyntaxHighlighting::Enabled, {
            HighlightMode::Streaming
        });
    }
    let again = highlight_code(
        "fn keep_me() {}\n",
        "rust",
        styles,
        SyntaxHighlighting::Enabled,
        HighlightMode::Committed,
    )
    .expect("rust grammar");
    assert!(
        std::sync::Arc::ptr_eq(&committed, &again),
        "committed LRU entry must survive streaming renders"
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
            SyntaxHighlighting::Enabled,
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
            SyntaxHighlighting::Disabled,
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
        SyntaxHighlighting::Enabled,
        HighlightMode::Committed,
    )
    .expect("a");
    let b = highlight_code(
        code,
        "rust",
        styles,
        SyntaxHighlighting::Enabled,
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
        SyntaxHighlighting::Enabled,
        HighlightMode::Committed,
    )
    .expect("a");
    let b = highlight_code(
        code,
        "rust",
        UiStyles::new(&t2),
        SyntaxHighlighting::Enabled,
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
        SyntaxHighlighting::Enabled,
        HighlightMode::Committed,
    )
    .expect("a");
    let b = highlight_code(
        "fn bbb_distinct() {}\n",
        "rust",
        styles,
        SyntaxHighlighting::Enabled,
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
    prewarm_highlighting();
    // Warmed grammars still highlight correctly afterwards.
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let highlighted = highlight_code(
        "# title\n",
        "md",
        styles,
        SyntaxHighlighting::Enabled,
        HighlightMode::Committed,
    )
    .expect("markdown highlights after prewarm");
    assert!(!highlighted.is_empty());
}
