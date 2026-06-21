use super::*;
use crate::i18n::locale_test_guard;
use crate::theme::Theme;
use coco_tui_ui::style::UiStyles;
use coco_types::ReasoningEffort;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn entry(provider: &str, provider_display: &str, model_id: &str, display_name: &str) -> ModelEntry {
    ModelEntry {
        provider: provider.to_string(),
        provider_display: provider_display.to_string(),
        model_id: model_id.to_string(),
        display_name: display_name.to_string(),
        context_window: Some(200_000),
        supported_efforts: Vec::new(),
        default_effort: None,
        is_current_for_role: false,
        unavailable_reasons: Vec::new(),
    }
}

fn sample_state() -> ModelPickerState {
    let mut current = entry(
        "anthropic",
        "Anthropic",
        "claude-sonnet-4-6",
        "Claude Sonnet 4.6",
    );
    current.supported_efforts = vec![
        ReasoningEffort::Auto,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
    ];
    current.default_effort = Some(ReasoningEffort::Auto);
    current.is_current_for_role = true;

    let mut unavailable = entry("openai", "OpenAI", "gpt-5.4", "GPT-5.4");
    unavailable.supported_efforts = vec![ReasoningEffort::Low, ReasoningEffort::High];
    unavailable.default_effort = Some(ReasoningEffort::Low);
    unavailable
        .unavailable_reasons
        .push(ProviderUnavailableReason::MissingApiKey {
            env_key: "OPENAI_API_KEY".to_string(),
        });

    ModelPickerState {
        role: ModelRole::Main,
        entries: vec![
            current,
            entry("anthropic", "Anthropic", "claude-haiku-4-5", "Claude Haiku"),
            entry("google", "Google", "gemini-2.5-pro", "Gemini 2.5 Pro"),
            unavailable,
        ],
        filter: String::new(),
        selected: 0,
        effort: Some(ReasoningEffort::Auto),
    }
}

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn lines_text(lines: &[Line<'_>]) -> String {
    lines.iter().map(line_text).collect::<Vec<_>>().join("\n")
}

fn render_snapshot(width: u16, height: u16, state: &ModelPickerState) -> String {
    let _locale = locale_test_guard("en");
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    let theme = Theme::default();
    terminal
        .draw(|frame| render_model_picker(frame, frame.area(), state, UiStyles::new(&theme)))
        .unwrap();
    let buf = terminal.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..height {
        for x in 0..width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

#[test]
fn groups_rows_by_provider() {
    let _locale = locale_test_guard("en");
    let state = sample_state();
    let view = build_view_model(&state, 20);

    assert!(matches!(view.list.rows[0], PickerRow::Header("Anthropic")));
    assert!(matches!(
        view.list.rows[1],
        PickerRow::Entry {
            filtered_index: 0,
            ..
        }
    ));
    assert!(matches!(view.list.rows[3], PickerRow::Blank));
    assert!(matches!(view.list.rows[4], PickerRow::Header("Google")));
    assert!(matches!(view.list.rows[6], PickerRow::Blank));
    assert!(matches!(view.list.rows[7], PickerRow::Header("OpenAI")));
}

#[test]
fn current_row_is_visible_and_badged() {
    let _locale = locale_test_guard("en");
    let state = sample_state();
    let theme = Theme::default();
    let lines = render_model_picker_lines(&state, UiStyles::new(&theme), 80, 18);
    let text = lines_text(&lines);

    assert!(text.contains("Claude Sonnet 4.6"));
    assert!(text.contains("[current]"));
}

#[test]
fn unavailable_provider_rows_stay_visible_with_reason() {
    let _locale = locale_test_guard("en");
    let mut state = sample_state();
    state.selected = 3;
    state.effort = Some(ReasoningEffort::Low);
    let theme = Theme::default();
    let lines = render_model_picker_lines(&state, UiStyles::new(&theme), 90, 18);
    let text = lines_text(&lines);

    assert!(text.contains("GPT-5.4"));
    assert!(text.contains("unavailable"));
    assert!(text.contains("missing API key; set OPENAI_API_KEY"));
}

#[test]
fn effort_line_handles_supported_and_unsupported_models() {
    let _locale = locale_test_guard("en");
    let mut state = sample_state();
    let theme = Theme::default();
    let supported = build_view_model(&state, 10);
    let supported_line = line_text(&render_effort_line(
        &state,
        &supported,
        UiStyles::new(&theme),
    ));
    assert!(supported_line.contains("▸auto◂"));
    assert!(supported_line.contains(" high "));

    state.selected = 1;
    state.effort = None;
    let unsupported = build_view_model(&state, 10);
    let unsupported_line = line_text(&render_effort_line(
        &state,
        &unsupported,
        UiStyles::new(&theme),
    ));
    assert!(unsupported_line.contains("Thinking:"));
    assert!(unsupported_line.contains("unavailable"));
}

#[test]
fn filtered_selection_uses_filtered_index() {
    let _locale = locale_test_guard("en");
    let mut state = sample_state();
    state.filter = "open".to_string();
    state.selected = 0;
    state.effort = Some(ReasoningEffort::Low);
    let theme = Theme::default();
    let lines = render_model_picker_lines(&state, UiStyles::new(&theme), 90, 18);
    let text = lines_text(&lines);

    assert!(text.contains("OpenAI"));
    assert!(text.contains("❯ GPT-5.4"));
    assert!(!text.contains("Claude Sonnet"));
}

#[test]
fn snapshot_model_picker_narrow() {
    let state = sample_state();
    insta::assert_snapshot!("model_picker_narrow", render_snapshot(50, 20, &state));
}

#[test]
fn snapshot_model_picker_normal() {
    let state = sample_state();
    insta::assert_snapshot!("model_picker_normal", render_snapshot(90, 24, &state));
}

#[test]
fn snapshot_model_picker_wide() {
    let state = sample_state();
    insta::assert_snapshot!("model_picker_wide", render_snapshot(140, 34, &state));
}
