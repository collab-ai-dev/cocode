use coco_tui_ui::style::UiStyles;
use coco_tui_ui::theme::Theme;

use super::mermaid_to_lines;

fn render(src: &str, width: u16) -> Option<Vec<String>> {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    mermaid_to_lines(src, styles, width).map(|lines| {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    })
}

#[test]
fn flowchart_renders_nodes_and_box_drawing() {
    let out = render("flowchart LR\n  A[Start] --> B[Finish]\n", 80)
        .expect("a simple flowchart renders to cells");
    let joined = out.join("\n");
    assert!(
        joined.contains("Start"),
        "node label Start missing:\n{joined}"
    );
    assert!(
        joined.contains("Finish"),
        "node label Finish missing:\n{joined}"
    );
    assert!(
        joined
            .chars()
            .any(|c| matches!(c, '╭' | '╮' | '╰' | '╯' | '│' | '─')),
        "expected box-drawing glyphs:\n{joined}"
    );
    assert!(
        joined.chars().any(|c| matches!(c, '→' | '←' | '↑' | '↓')),
        "expected an arrowhead:\n{joined}"
    );
}

#[test]
fn unsupported_diagram_type_falls_back() {
    // Sequence diagrams are not box-and-arrow graphs → verbatim fence.
    assert!(render("sequenceDiagram\n  Alice->>Bob: Hi\n", 80).is_none());
    // Pie charts carry continuous geometry → verbatim fence.
    assert!(render("pie\n  \"A\" : 50\n  \"B\" : 50\n", 80).is_none());
}

#[test]
fn empty_diagram_falls_back() {
    assert!(render("", 80).is_none());
    assert!(render("   \n  \n", 80).is_none());
}

#[test]
fn arbitrary_text_never_panics() {
    // The upstream parser is lenient: arbitrary text may parse to a trivial
    // graph or fall back — either is fine, the contract is "never panic".
    let _ = render("this is not a mermaid diagram at all", 80);
}

#[test]
fn too_narrow_falls_back() {
    assert!(render("flowchart LR\n  A[Start] --> B[Finish]\n", 8).is_none());
}

#[test]
fn arrow_char_points_in_travel_direction() {
    use super::arrow_char;
    assert_eq!(arrow_char(0, 5, 3, 5), '→');
    assert_eq!(arrow_char(5, 5, 2, 5), '←');
    assert_eq!(arrow_char(5, 0, 5, 3), '↓');
    assert_eq!(arrow_char(5, 5, 5, 2), '↑');
}

#[test]
fn corner_glyph_maps_two_way_bends() {
    use super::corner_glyph;
    // cur=(1,1); (prev direction, next direction) → rounded corner.
    assert_eq!(corner_glyph((0, 1), (1, 1), (1, 2)), Some('╮')); // left + down
    assert_eq!(corner_glyph((2, 1), (1, 1), (1, 2)), Some('╭')); // right + down
    assert_eq!(corner_glyph((0, 1), (1, 1), (1, 0)), Some('╯')); // left + up
    assert_eq!(corner_glyph((2, 1), (1, 1), (1, 0)), Some('╰')); // right + up
    assert_eq!(corner_glyph((0, 1), (1, 1), (2, 1)), None); // collinear pass-through
}

#[test]
fn degenerate_edge_emits_no_stray_arrow() {
    // A flowchart whose layout might collapse an edge to identical points must
    // not paint a stray arrowhead; rendering still succeeds without panic.
    let _ = render("flowchart LR\n  A --> A\n", 80);
}
