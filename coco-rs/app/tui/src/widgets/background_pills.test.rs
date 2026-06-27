use super::BackgroundPills;
use super::BackgroundPillsView;
use super::PillEntry;
use crate::theme::Theme;
use coco_tui_ui::style::UiStyles;
use pretty_assertions::assert_eq;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::widgets::Widget;

fn render(view: BackgroundPillsView<'_>, w: u16, h: u16) -> String {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let widget = BackgroundPills::new(&view, styles);
    let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test backend");
    terminal
        .draw(|f| widget.render(f.area(), f.buffer_mut()))
        .expect("draw");
    let buf = terminal.backend().buffer();
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn empty_view_renders_nothing() {
    let out = render(BackgroundPillsView::default(), 80, 1);
    assert_eq!(out, "");
}

#[test]
fn single_running_pill_uses_at_prefix() {
    let view = BackgroundPillsView {
        pills: vec![PillEntry {
            label: "alex",
            is_idle: false,
            color: None,
        }],
    };
    let out = render(view, 80, 1);
    assert_eq!(out, "@alex", "pill should render as `@name` with no frame");
}

#[test]
fn multiple_pills_join_with_single_space() {
    let view = BackgroundPillsView {
        pills: vec![
            PillEntry {
                label: "alex",
                is_idle: false,
                color: None,
            },
            PillEntry {
                label: "blake",
                is_idle: false,
                color: None,
            },
            PillEntry {
                label: "casey",
                is_idle: true,
                color: None,
            },
        ],
    };
    let out = render(view, 80, 1);
    assert_eq!(out, "@alex @blake @casey");
}

#[test]
fn overflow_emits_plus_n_more_tail() {
    let view = BackgroundPillsView {
        pills: (0..6)
            .map(|_| PillEntry {
                label: "agentname",
                is_idle: false,
                color: None,
            })
            .collect(),
    };
    // 40 columns: fits a couple of `@agentname` pills + `[+N more]`.
    let out = render(view, 40, 1);
    assert!(out.contains("+"), "overflow tail missing: {out:?}");
    assert!(out.contains("more"), "overflow word missing: {out:?}");
}

#[test]
fn narrow_width_keeps_first_pill_visible() {
    let view = BackgroundPillsView {
        pills: vec![PillEntry {
            label: "tiny",
            is_idle: false,
            color: None,
        }],
    };
    // `@tiny` is 5 cols; 20 cols is plenty even with overflow reserve
    // (single pill never overflows because the trailing reserve is
    // gated on `i + 1 < pills.len()`).
    let out = render(view, 20, 1);
    assert_eq!(out, "@tiny");
}
