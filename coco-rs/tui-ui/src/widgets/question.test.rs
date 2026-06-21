use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

use super::ActionRow;
use super::ChoiceRow;
use super::InputRow;
use super::QuestionHeader;
use super::QuestionRow;
use super::QuestionView;
use super::QuestionWidget;
use super::RowMark;
use crate::style::UiStyles;
use crate::theme::Theme;

fn radio(number: usize, label: &str, description: &str, focused: bool) -> QuestionRow {
    QuestionRow::Choice(ChoiceRow {
        number,
        label: label.into(),
        description: description.into(),
        mark: RowMark::Radio {
            selected: number == 1,
            focused,
        },
    })
}

fn check(number: usize, label: &str, checked: bool, focused: bool) -> QuestionRow {
    QuestionRow::Choice(ChoiceRow {
        number,
        label: label.into(),
        description: String::new(),
        mark: RowMark::Check { checked, focused },
    })
}

fn render(view: &QuestionView, width: u16, height: u16) -> Vec<String> {
    let theme = Theme::default();
    let area = Rect::new(0, 0, width, height);
    let mut buf = Buffer::empty(area);
    QuestionWidget::new(view, UiStyles::new(&theme)).render(area, &mut buf);
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

fn base() -> QuestionView {
    QuestionView {
        header: QuestionHeader {
            title: " Question ".into(),
            chip: Some("Auth".into()),
            nav: None,
        },
        body: "Which auth flow?".into(),
        rows: vec![
            radio(1, "OAuth", "browser login", true),
            QuestionRow::Input(InputRow {
                number: 2,
                label: "Type something.".into(),
                value: String::new(),
                selected: false,
                focused: false,
            }),
        ],
        submit_review: None,
        preview: None,
        footer_actions: vec![ActionRow {
            number: 3,
            label: "Chat about this".into(),
            focused: false,
        }],
        hints: "up/down navigate".into(),
    }
}

#[test]
fn single_select_renders_chip_numbers_cursor_and_footer() {
    let joined = render(&base(), 60, 20).join("\n");
    assert!(joined.contains("[Auth]"), "single-question chip:\n{joined}");
    assert!(joined.contains("1. OAuth"), "numbered option:\n{joined}");
    assert!(
        joined.contains("2. Type something."),
        "input row:\n{joined}"
    );
    assert!(joined.contains('❯'), "focus cursor:\n{joined}");
    assert!(joined.contains("browser login"), "description:\n{joined}");
    assert!(joined.contains("3. Chat about this"), "footer:\n{joined}");
    assert!(joined.contains("OAuth ✔"), "selected marker:\n{joined}");
    let lines = render(&base(), 60, 20);
    let chat_idx = lines
        .iter()
        .position(|line| line.contains("3. Chat about this"))
        .expect("chat row");
    assert_eq!(
        lines.get(chat_idx + 1).map(String::as_str),
        Some(""),
        "footer action and hints should be separated:\n{joined}"
    );
    assert!(
        !joined.contains('┌') && !joined.contains('│'),
        "question prompt should be unbordered:\n{joined}"
    );
}

#[test]
fn multi_question_nav_strip_renders_tabs_arrows_and_checkboxes() {
    let mut view = base();
    view.header.chip = None;
    view.header.nav = Some(super::QuestionNav {
        tabs: vec![
            super::NavTab {
                header: "Auth".into(),
                answered: true,
            },
            super::NavTab {
                header: "Tools".into(),
                answered: false,
            },
        ],
        current: 0,
        submit: Some(super::SubmitNavTab {
            focused: false,
            ready: false,
        }),
    });
    let joined = render(&view, 60, 22).join("\n");
    // All headers appear in the strip, with ☒ for answered / ☐ for not.
    assert!(joined.contains("☒ Auth"), "answered tab:\n{joined}");
    assert!(joined.contains("☐ Tools"), "unanswered tab:\n{joined}");
    // The trailing Submit tab is shown (☐ until every question is answered).
    assert!(joined.contains("Submit"), "submit tab:\n{joined}");
    // Navigation arrows frame the strip.
    assert!(
        joined.contains('←') && joined.contains('→'),
        "arrows:\n{joined}"
    );
    // The bare single-question chip is gone when the strip is shown.
    assert!(
        !joined.contains("[Auth]"),
        "no bare chip with nav:\n{joined}"
    );
}

#[test]
fn nav_strip_submit_tab_shows_check_when_ready_and_focused() {
    let mut view = base();
    view.header.chip = None;
    view.header.nav = Some(super::QuestionNav {
        tabs: vec![super::NavTab {
            header: "Q1".into(),
            answered: true,
        }],
        current: 0,
        submit: Some(super::SubmitNavTab {
            focused: true,
            ready: true,
        }),
    });
    let joined = render(&view, 60, 22).join("\n");
    assert!(
        joined.contains("✔ Submit"),
        "ready submit tab shows ✔:\n{joined}"
    );
}

#[test]
fn multi_select_renders_checkboxes() {
    let mut view = base();
    view.rows = vec![
        check(1, "Read", true, true),
        check(2, "Write", false, false),
    ];
    let joined = render(&view, 60, 16).join("\n");
    assert!(joined.contains("[x] Read"), "checked box:\n{joined}");
    assert!(joined.contains("[ ] Write"), "unchecked box:\n{joined}");
}

#[test]
fn free_text_input_renders_answer_buffer_with_caret() {
    let mut view = base();
    view.rows.push(QuestionRow::Input(InputRow {
        number: 2,
        label: "Type something.".into(),
        value: "device code".into(),
        selected: true,
        focused: true,
    }));
    let joined = render(&view, 60, 18).join("\n");
    assert!(joined.contains("device code▌"), "input line:\n{joined}");
}

#[test]
fn focused_empty_free_text_replaces_placeholder_with_caret() {
    let mut view = base();
    view.rows = vec![QuestionRow::Input(InputRow {
        number: 1,
        label: "Type something.".into(),
        value: String::new(),
        selected: false,
        focused: true,
    })];

    let joined = render(&view, 60, 14).join("\n");
    assert!(
        !joined.contains("Type something."),
        "focused empty input should not show placeholder:\n{joined}"
    );
    assert!(joined.contains("❯ 1. ▌"), "focused input caret:\n{joined}");
}

#[test]
fn free_text_input_wraps_long_unbroken_values() {
    let mut view = base();
    view.rows = vec![QuestionRow::Input(InputRow {
        number: 1,
        label: "Type something.".into(),
        value: "abcdefghijklmnopqrstuvwxyz0123456789".into(),
        selected: true,
        focused: true,
    })];

    let mut short = base();
    short.rows = vec![QuestionRow::Input(InputRow {
        number: 1,
        label: "Type something.".into(),
        value: "abc".into(),
        selected: true,
        focused: true,
    })];
    let short_height = short.desired_height(30, UiStyles::new(&Theme::default()));
    let wrapped_height = view.desired_height(30, UiStyles::new(&Theme::default()));
    assert!(
        wrapped_height > short_height,
        "long free-text input should add wrapped body rows"
    );
    let joined = render(&view, 30, 20).join("\n");
    assert!(joined.contains("abcdefghij"), "first chunk:\n{joined}");
    assert!(joined.contains("yz012345"), "later chunk:\n{joined}");
}

#[test]
fn wide_preview_renders_side_by_side_with_options() {
    let mut view = base();
    view.preview = Some("flowchart:\n  user to token".into());
    let rows = render(&view, 96, 16);
    let joined = rows.join("\n");
    // Both the option list and the preview header must be present.
    assert!(joined.contains("1. OAuth"), "options present:\n{joined}");
    assert!(
        joined.contains("preview"),
        "preview header present:\n{joined}"
    );
    assert!(
        joined.contains("flowchart"),
        "preview body present:\n{joined}"
    );
    // Side-by-side: the body and the preview share a row below the fixed header.
    let has_side_by_side = rows
        .iter()
        .any(|r| r.contains("Which auth flow?") && r.contains("preview"));
    assert!(has_side_by_side, "expected a side-by-side row:\n{joined}");
}

#[test]
fn narrow_preview_stacks_under_options() {
    let mut view = base();
    view.preview = Some("stacked preview body".into());
    let rows = render(&view, 50, 22);
    let joined = rows.join("\n");
    assert!(joined.contains("preview"), "stacked marker:\n{joined}");
    assert!(
        joined.contains("stacked preview body"),
        "stacked body:\n{joined}"
    );
    // No row mixes an option and the preview (single column).
    let mixed = rows
        .iter()
        .any(|r| r.contains("OAuth") && r.contains("stacked preview body"));
    assert!(!mixed, "must be single-column:\n{joined}");
}
