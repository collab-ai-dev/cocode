use super::*;
use crate::i18n::locale_test_guard;
use crate::state::ProviderTemplate;
use crate::state::ProviderWizardState;
use crate::state::ProviderWizardStep;
use crate::theme::Theme;
use coco_tui_ui::style::UiStyles;
use coco_types::ProviderApi;
use coco_types::WireApi;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn sample() -> ProviderWizardState {
    ProviderWizardState::new(vec![
        ProviderTemplate {
            name: "anthropic".to_string(),
            api: ProviderApi::Anthropic,
            base_url: "https://api.anthropic.com".to_string(),
            wire_api: WireApi::Chat,
            env_key: "ANTHROPIC_API_KEY".to_string(),
            is_custom: false,
        },
        ProviderTemplate {
            name: "openai".to_string(),
            api: ProviderApi::Openai,
            base_url: "https://api.openai.com/v1".to_string(),
            wire_api: WireApi::Chat,
            env_key: "OPENAI_API_KEY".to_string(),
            is_custom: false,
        },
        ProviderTemplate::custom(),
    ])
}

fn render_snapshot(state: &ProviderWizardState) -> String {
    use ratatui::widgets::Block;
    use ratatui::widgets::Borders;
    use ratatui::widgets::Padding;
    use ratatui::widgets::Paragraph;
    let _locale = locale_test_guard("en");
    let (w, h) = (72u16, 18u16);
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    terminal
        .draw(|frame| {
            let area = frame.area();
            let box_width = area.width.clamp(40, 80);
            let inner_width = box_width.saturating_sub(4).max(1);
            let list_visible = area.height.saturating_sub(8).max(3) as usize;
            let lines = provider_wizard_lines(state, styles, inner_width as usize, list_visible);
            let box_height = (lines.len() as u16 + 2).min(area.height);
            let modal_area = Rect {
                x: 0,
                y: 0,
                width: box_width,
                height: box_height,
            };
            frame.render_widget(
                Paragraph::new(lines).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .padding(Padding::horizontal(1))
                        .title(provider_wizard_title())
                        .border_style(Style::default().fg(styles.modal_border())),
                ),
                modal_area,
            );
        })
        .unwrap();
    let buf = terminal.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..h {
        for x in 0..w {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

#[test]
fn snapshot_template_step() {
    let state = sample();
    insta::assert_snapshot!("provider_wizard_template", render_snapshot(&state));
}

#[test]
fn snapshot_confirm_step() {
    let mut state = sample();
    state.template_idx = 1;
    state.name = WizardTextField::seeded("my-openai");
    state.api_key = WizardTextField::seeded("sk-abc123");
    state.step = ProviderWizardStep::Confirm;
    insta::assert_snapshot!("provider_wizard_confirm", render_snapshot(&state));
}

#[test]
fn snapshot_api_key_step_masks_input() {
    let mut state = sample();
    state.api_key = WizardTextField::seeded("secret");
    state.step = ProviderWizardStep::ApiKey;
    let out = render_snapshot(&state);
    // The key is masked with bullets, never shown verbatim.
    assert!(out.contains('•'));
    assert!(!out.contains("secret"));
}
