use coco_tui_ui::display::SyntaxHighlighting;
use coco_tui_ui::style::UiStyles;
use coco_tui_ui::theme::Theme;

use super::*;

#[test]
fn test_stream_render_controller_reuses_stable_prefix_for_new_tail() {
    let theme = Theme::default();
    let mut controller = StreamRenderController::default();

    let first = render_all(&mut controller, input("first\n\nsecond", &theme));
    let stable_after_first = controller.stable_prefix_end;
    let second = render_all(&mut controller, input("first\n\nsecond\n\nthird", &theme));

    assert!(stable_after_first > 0);
    assert_eq!(controller.stable_prefix_end, "first\n\nsecond\n\n".len());
    assert!(second.len() >= first.len());
    assert_eq!(
        controller.stable_rendered_source_bytes, controller.stable_prefix_end,
        "each stable source byte must be rendered exactly once"
    );
}

#[test]
fn test_stream_render_controller_render_does_not_duplicate_new_stable_lines() {
    let theme = Theme::default();
    let mut controller = StreamRenderController::default();

    let rendered = render_all(&mut controller, input("alpha\n\nbeta", &theme));
    let text = rendered
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(text.matches("alpha").count(), 1, "{text}");
    assert_eq!(text.matches("beta").count(), 1, "{text}");
}

#[test]
fn stable_stream_links_follow_terminal_capability() {
    let theme = Theme::default();
    let source = "See [docs](https://example.com/docs).\n\n";

    let mut fallback_controller = StreamRenderController::default();
    let fallback = fallback_controller.render_projection(input(source, &theme));
    let fallback_text = fallback
        .stable_lines
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(fallback_text.contains("docs (https://example.com/docs)"));
    assert!(fallback.stable_links.is_empty());

    let mut sidecar_controller = StreamRenderController::default();
    let sidecar = sidecar_controller.render_projection(StreamRenderInput {
        source,
        generation: 1,
        styles: UiStyles::new(&theme),
        width: 80,
        syntax_highlighting: SyntaxHighlighting::Off,
        hyperlinks_enabled: true,
    });
    let sidecar_text = sidecar
        .stable_lines
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!sidecar_text.contains("https://example.com/docs"));
    assert_eq!(sidecar.stable_links.len(), 1);
    assert_eq!(sidecar.stable_links[0].target, "https://example.com/docs");
}

/// Stable + tail concatenated, as the retired full-render entry point did.
fn render_all(
    controller: &mut StreamRenderController,
    input: StreamRenderInput<'_>,
) -> Vec<ratatui::text::Line<'static>> {
    let projection = controller.render_projection(input);
    let mut lines = Vec::with_capacity(projection.stable_lines.len() + projection.tail_lines.len());
    lines.extend(projection.stable_lines.iter().cloned());
    lines.extend(projection.tail_lines.iter().cloned());
    lines
}

fn input<'a>(source: &'a str, theme: &'a Theme) -> StreamRenderInput<'a> {
    // Every test input gets a fresh generation: the production contract is
    // "equal generation ⇒ identical source", and tests feed evolving sources.
    static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    StreamRenderInput {
        source,
        generation: GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        styles: UiStyles::new(theme),
        width: 80,
        syntax_highlighting: SyntaxHighlighting::Off,
        hyperlinks_enabled: false,
    }
}

fn line_text(line: &ratatui::text::Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn test_stable_lines_are_row_prefix_of_full_committed_render() {
    // THE soundness pin for the anchored finalize (tui-v2 §6.2): with the
    // rasterized fingerprint compare deleted, the finalize appends
    // `render_committed(full_text)[line_prefix_len..]` directly after rows
    // produced from `render_committed(stable_prefix)`. The committed-scrollback
    // soundness argument is `stable(k) ⊑ stable(final) ⊑ full(final)`. This one
    // pin now closes BOTH links across the production config matrix:
    //
    //   - row-prefix:   stable(k) ⊑ full(k)              (the finalize relation)
    //   - append-only:  stable(k-1) ⊑ stable(k)          (emitted rows immutable)
    //
    // over the FULL trap set — closed fence, mermaid diagram, growing loose
    // list, setext underline arriving after its paragraph, blockquote, GFM
    // table, indented code, a blank-containing raw HTML block, multiline and
    // late-defined reference links, trailing partial line — at a wide and a
    // narrow width, and under BOTH syntax states (production defaults to
    // Enabled, and highlighted fences are the highest-risk construct). A
    // failure means streamed scrollback rows would disagree with the finalize
    // suffix: silent transcript corruption.
    let theme = Theme::default();
    let source = "Intro paragraph.\n\n```rust\nfn main() {}\n```\n\n\
                  ```mermaid\ngraph TD\n  A-->B\n```\n\n- alpha item\n\n\
                  - beta item\n\nTitle\n=====\n\n> quoted line\n> second line\n\n\
                  | col a | col b |\n| ----- | ----- |\n| one   | two   |\n\n\
                  Indented code follows:\n\n    first line\n\n    second line\n\n\
                  <script>\nconst first = 1;\n\nconst second = 2;\n</script>\n\n\
                  See [a multiline\nlabel][ref] too.\n\n\
                  See [the spec][ref] for details.\n\n[ref]: https://example.com\n\n\
                  Closing paragraph.\n\ntrailing partial line";
    for syntax in [SyntaxHighlighting::Full, SyntaxHighlighting::Off] {
        for width in [80u16, 24] {
            let mut controller = StreamRenderController::default();
            let mut prev_stable: Vec<String> = Vec::new();
            let mut fed = 0;
            while fed < source.len() {
                fed = (fed + 7).min(source.len());
                let view = &source[..fed];
                let projection = controller.render_projection(StreamRenderInput {
                    source: view,
                    generation: fed as u64,
                    styles: UiStyles::new(&theme),
                    width,
                    syntax_highlighting: syntax,
                    hyperlinks_enabled: false,
                });
                let stable: Vec<String> = projection
                    .stable_lines
                    .iter()
                    .map(|line| format!("{line:?}"))
                    .collect();
                let stable_prefix: Vec<String> = if projection.stable_source_len == 0 {
                    Vec::new()
                } else {
                    crate::transcript::render::assistant::render_committed_assistant_markdown(
                        &view[..projection.stable_source_len],
                        crate::transcript::render::assistant::CommittedAssistantMarkdownOptions {
                            styles: UiStyles::new(&theme),
                            width,
                            syntax_highlighting: syntax,
                        },
                    )
                    .iter()
                    .map(|line| format!("{line:?}"))
                    .collect()
                };
                let full: Vec<String> =
                    crate::transcript::render::assistant::render_committed_assistant_markdown(
                        view,
                        crate::transcript::render::assistant::CommittedAssistantMarkdownOptions {
                            styles: UiStyles::new(&theme),
                            width,
                            syntax_highlighting: syntax,
                        },
                    )
                    .iter()
                    .map(|line| format!("{line:?}"))
                    .collect();
                assert!(
                    stable.len() <= full.len() && full[..stable.len()] == stable[..],
                    "stable-prefix render must be a row-prefix of the committed full render (syntax={syntax:?}, width={width}, fed={fed}):\nstable {stable:#?}\nfull {full:#?}",
                );
                assert_eq!(
                    stable, stable_prefix,
                    "independently rendered stable regions must equal one committed prefix render (syntax={syntax:?}, width={width}, fed={fed})",
                );
                assert!(
                    stable.len() >= prev_stable.len()
                        && stable[..prev_stable.len()] == prev_stable[..],
                    "stable rows must be append-only across advances on the trap source (syntax={syntax:?}, width={width}, fed={fed}):\nwas {prev_stable:#?}\nnow {stable:#?}",
                );
                prev_stable = stable;
            }
        }
    }
}

#[test]
fn test_context_free_long_stream_renders_each_stable_source_byte_once() {
    let theme = Theme::default();
    let mut controller = StreamRenderController::default();
    let mut source = String::new();
    for block in 0..500 {
        source.push_str(&format!("paragraph {block}\n\n"));
        controller.render_projection(input(&source, &theme));
    }

    assert_eq!(controller.stable_prefix_end, source.len());
    assert_eq!(
        controller.stable_rendered_source_bytes,
        source.len(),
        "the common context-free path must render disjoint stable slices"
    );
    let authoritative = crate::transcript::render::assistant::render_committed_assistant_markdown(
        &source,
        crate::transcript::render::assistant::CommittedAssistantMarkdownOptions {
            styles: UiStyles::new(&theme),
            width: 80,
            syntax_highlighting: SyntaxHighlighting::Off,
        },
    );
    assert_eq!(controller.stable_lines, authoritative);
}

#[test]
fn test_reference_definitions_use_authoritative_full_render_fallback() {
    let theme = Theme::default();
    let mut controller = StreamRenderController::default();
    let definition_only = "[target]: https://example.com\n\n";
    let first = controller.render_projection(input(definition_only, &theme));
    assert!(
        first.stable_lines.is_empty(),
        "do not commit a marker-only prefix"
    );
    assert_eq!(first.stable_source_len, 0);

    let source = format!("{definition_only}See [the target][target].\n\n");
    let projection = controller.render_projection(input(&source, &theme));
    let authoritative = crate::transcript::render::assistant::render_committed_assistant_markdown(
        &source,
        crate::transcript::render::assistant::CommittedAssistantMarkdownOptions {
            styles: UiStyles::new(&theme),
            width: 80,
            syntax_highlighting: SyntaxHighlighting::Off,
        },
    );
    assert_eq!(projection.stable_lines, authoritative);
    assert_eq!(projection.stable_source_len, source.len());
}

#[test]
fn test_stable_lines_remain_prefix_stable_across_advances() {
    // Regression for the loose-list flip (2026-06-10 production
    // `pending_stream_prefix_rows_mismatch` replay): items separated by blank
    // lines arrive incrementally; every already-stable line must survive every
    // later advance byte-identically, because emitted scrollback rows can
    // never be rewritten.
    let theme = Theme::default();
    let source = "Intro paragraph.\n\n- alpha item\n\n- beta item\n\n- gamma item\n\n\
                  Closing paragraph.\n\n### Next section\n\nmore text\n\n";
    let mut controller = StreamRenderController::default();
    let mut prev: Vec<String> = Vec::new();
    let mut fed = 0;
    while fed < source.len() {
        fed = (fed + 7).min(source.len());
        let projection = controller.render_projection(input(&source[..fed], &theme));
        let cur: Vec<String> = projection
            .stable_lines
            .iter()
            .map(|line| format!("{line:?}"))
            .collect();
        assert!(
            cur.len() >= prev.len() && cur[..prev.len()] == prev[..],
            "stable lines must be append-only across advances (fed={fed}):\nwas {prev:#?}\nnow {cur:#?}",
        );
        prev = cur;
    }
}
